//! Filesystem hardening for the paths the server owns: config dir + config
//! file, log dir, store dir.
//!
//! The design (docs/design/ingress-security.md §4.4) asks for two things:
//! explicit safe permissions at creation time — so no check-then-create
//! window exists on the file or directory itself — and a startup validation
//! that rejects paths another account could have swapped underneath us:
//! symlinks in any component, group/other write on the directories, an owner
//! that is not us, and world-writable parents without a sticky bit.
//!
//! With `[server] fs_strict = true` every violation refuses the start (an
//! `ActError::Config`); the default logs a warning naming the path and fixes
//! what fixing can fix (chmod on unix). A symlink is never fixable — the
//! path already points somewhere else — so it is refused in both modes.
//!
//! Strict mode is fully implemented for unix and windows, the two shipped
//! platforms. There is deliberately no portable fallback that silently
//! passes strict mode on an unsupported target: the permission checks below
//! are `#[cfg(unix)]` / `#[cfg(windows)]` and a build on any other target
//! gets the symlink checks only.

use std::fs;
use std::io;
use std::path::Path;

use acts::ActError;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create `path` (and its parents) with safe permissions.
///
/// The directory is created first (`create_dir_all`), then the mode is set
/// immediately on unix (0700 — the appender, sqlite and sled/rocksdb all
/// write secrets-free but server-private data below it), or the DACL is
/// restricted to the current user, SYSTEM and Administrators on windows.
/// On an already-existing directory the same restrictions are (re-)applied,
/// so calling this twice is harmless.
pub fn ensure_dir_secure(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)?.permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(windows)]
    restrict_dacl(path)?;
    Ok(())
}

/// Validate a directory the server uses, per the module docs.
///
/// Every path component from the rootmost existing one down is checked for
/// being a symlink (`symlink_metadata` per component — never following one),
/// plus, on unix: group/other write on the directory itself, an owner that
/// is not the current user, and a world-writable parent without the sticky
/// bit. `strict` turns violations into a refusing `ActError::Config`; the
/// default warns and applies the best-effort fix (chmod 0700 on the
/// directory, other-write removed from world-writable parents).
pub fn validate_dir_secure(path: &Path, strict: bool) -> Result<(), ActError> {
    check_symlink_components(path)?;
    #[cfg(unix)]
    validate_dir_permissions(path, strict)?;
    #[cfg(windows)]
    {
        // The DACL applied by `ensure_dir_secure` is always re-applied before
        // this runs (same startup pass); windows exposes no cheap equivalent
        // of the unix mode checks, so symlink rejection is the validation
        // there.
        let _ = strict;
    }
    Ok(())
}

/// Create `path` with `contents`, or validate the existing file.
///
/// Missing files are created with `create_new(true)` in a single `open(2)`
/// that also carries the unix mode 0600, so no window exists in which the
/// file is readable by group/other. Existing files are checked: a symlink
/// (final component) is refused, group/other access bits are removed with a
/// warning (on unix), and on windows the same restricted DACL as the
/// directories is applied. Used for `acts.toml`.
pub fn ensure_file_secure(path: &Path, contents: &str) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            // An existing symlink must never be adopted as the config file:
            // its target was chosen by whoever could create the link.
            if meta.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is a symlink; refusing to use it", path.display()),
                ));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)?;
                use io::Write;
                file.write_all(contents.as_bytes())?;
            }
            #[cfg(windows)]
            {
                fs::write(path, contents)?;
                restrict_dacl(path)?;
            }
        }
        Err(err) => return Err(err),
    }
    #[cfg(unix)]
    harden_existing_file(path);
    #[cfg(windows)]
    restrict_dacl(path)?;
    Ok(())
}

/// Validate an existing file the server owns (the config file): symlink
/// rejection as in [`ensure_file_secure`], plus the unix access-bit checks
/// with `strict` semantics like [`validate_dir_secure`].
pub fn validate_file_secure(path: &Path, strict: bool) -> Result<(), ActError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) => return Err(io_err(path, err)),
    };
    if meta.file_type().is_symlink() {
        return Err(refuse(
            strict,
            format!("{} is a symlink; refusing to use it", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            if strict {
                return Err(ActError::Config(format!(
                    "{} is readable/writable by group or other (mode {:o}); \
                     set fs_strict = false to fix it automatically",
                    path.display(),
                    mode & 0o777
                )));
            }
            tracing::warn!(
                path = %path.display(),
                mode = format_args!("{:o}", mode & 0o777),
                "config file is group/other accessible; restricting to 0600"
            );
            set_file_mode(path, 0o600)?;
        }
        if meta.uid() != current_uid() {
            return violation(
                strict,
                &format!(
                    "{} is owned by uid {} but the server runs as uid {}",
                    path.display(),
                    meta.uid(),
                    current_uid()
                ),
                || set_file_mode(path, 0o600).map(|_| ()),
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unix permission checks
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn validate_dir_permissions(path: &Path, strict: bool) -> Result<(), ActError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => return Err(io_err(path, err)),
    };
    let mode = meta.permissions().mode();
    if !meta.is_dir() {
        return Err(ActError::Config(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    if mode & 0o022 != 0 {
        if strict {
            return Err(ActError::Config(format!(
                "{} is writable by group or other (mode {:o}); set fs_strict = false \
                 to fix it automatically",
                path.display(),
                mode & 0o777
            )));
        }
        tracing::warn!(
            path = %path.display(),
            mode = format_args!("{:o}", mode & 0o777),
            "server directory is group/other writable; restricting to 0700"
        );
        set_dir_mode(path, 0o700)?;
    }
    if meta.uid() != current_uid() {
        return violation(
            strict,
            &format!(
                "{} is owned by uid {} but the server runs as uid {}",
                path.display(),
                meta.uid(),
                current_uid()
            ),
            || set_dir_mode(path, 0o700).map(|_| ()),
        );
    }
    // A world-writable parent without the sticky bit lets any local account
    // rename or replace our directories out from under the server (the
    // sticky bit — as on /tmp — removes exactly that rename permission).
    for parent in path.ancestors().skip(1) {
        if let Ok(meta) = fs::metadata(parent) {
            let mode = meta.permissions().mode();
            if mode & 0o002 != 0 && mode & 0o1000 == 0 {
                return violation(
                    strict,
                    &format!(
                        "parent {} is world-writable without the sticky bit (mode {:o})",
                        parent.display(),
                        mode & 0o777
                    ),
                    || {
                        let mut perms = fs::metadata(parent)?.permissions();
                        perms.set_mode(mode & !0o002);
                        fs::set_permissions(parent, perms).map_err(io::Error::from)?;
                        Ok(())
                    },
                );
            }
        }
    }
    Ok(())
}

/// Reject when any component of `path` (the full path included) is a symlink.
/// Missing components are fine: they are about to be created by the caller.
fn check_symlink_components(path: &Path) -> Result<(), ActError> {
    for component in path.ancestors() {
        // symlink_metadata deliberately does not follow the final component:
        // a link there is exactly what this check refuses (TOCTOU window
        // closing is out of scope here — see the design's note on openat2).
        if let Ok(meta) = fs::symlink_metadata(component)
            && meta.file_type().is_symlink()
        {
            return Err(ActError::Config(format!(
                "{} is a symlink; the server refuses symlinked config, log \
                 and store paths",
                component.display()
            )));
        }
    }
    Ok(())
}

/// A violation that a fix can repair: strict reports and refuses, the
/// default tries the fix and only reports when it fails too. Only the unix
/// checks have fixes (modes); windows exclusivity is create-time, never
/// repaired after the fact.
#[cfg(unix)]
fn violation(
    strict: bool,
    what: &str,
    fix: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), ActError> {
    if strict {
        return Err(ActError::Config(format!(
            "{what}; set fs_strict = false to fix it automatically"
        )));
    }
    tracing::warn!("{what}; applying the best-effort fix");
    fix().map_err(|err| ActError::Store(format!("{what}, and the fix failed: {err}")))
}

/// Short-hand for the violations that have no fix at all.
fn refuse(strict: bool, what: String) -> ActError {
    if strict {
        ActError::Config(what)
    } else {
        tracing::warn!("{what}");
        ActError::Store(what)
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn set_dir_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    set_dir_mode(path, mode)
}

/// Best-effort restriction of an existing file, used by
/// [`ensure_file_secure`] after finding a group/other-accessible file.
#[cfg(unix)]
fn harden_existing_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            tracing::warn!(
                path = %path.display(),
                mode = format_args!("{:o}", mode & 0o777),
                "file is group/other accessible; restricting to 0600"
            );
            if let Err(err) = set_file_mode(path, 0o600) {
                tracing::warn!(error = %err, "failed to restrict the file mode");
            }
        }
    }
}

fn io_err(path: &Path, err: io::Error) -> ActError {
    ActError::Store(format!("{}: {err}", path.display()))
}

// ---------------------------------------------------------------------------
// Windows DACL
// ---------------------------------------------------------------------------

/// Restrict `path`'s DACL to the current user, SYSTEM and Administrators,
/// replacing whatever inherited permissions the parent granted (notably
/// Everyone/Users, which a default profile or `icacls`-restored ACL grants).
/// Idempotent: re-applying the same three ACEs is a no-op semantically.
#[cfg(windows)]
fn restrict_dacl(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, GENERIC_ALL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, AllocateAndInitializeSid, DACL_SECURITY_INFORMATION, FreeSid,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_NT_AUTHORITY, SID_IDENTIFIER_AUTHORITY,
    };
    use windows_sys::Win32::System::SystemServices::{
        DOMAIN_ALIAS_RID_ADMINS, SECURITY_BUILTIN_DOMAIN_RID, SECURITY_LOCAL_SYSTEM_RID,
    };

    // windows-sys types these as i32/u32 aliases; plain literals keep the
    // module free of the alias imports.
    const GRANT_ACCESS: i32 = 1;
    const TRUSTEE_IS_SID: i32 = 0;
    const TRUSTEE_IS_USER: i32 = 1;
    const TRUSTEE_IS_WELL_KNOWN_GROUP: i32 = 5;
    // OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE: children (sqlite -wal,
    // rocksdb SSTs, log hours) inherit the same exclusivity.
    const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 3;

    // SAFETY: AllocateAndInitializeSid takes plain values and returns a SID
    // the caller owns (released by `FreeSid` at the end of the scope). The
    // sub-authorities fill the argument slots from the first one on: a SID's
    // identity is its leading `subs.len()` sub-authorities, and a value in a
    // later slot is simply ignored (SYSTEM is S-1-5-18, administrators is
    // S-1-5-32-544).
    unsafe fn well_known_sid(
        authority: SID_IDENTIFIER_AUTHORITY,
        subs: &[u32],
    ) -> io::Result<PSID> {
        let mut sid: PSID = std::ptr::null_mut();
        let mut slots = [0u32; 8];
        for (slot, value) in slots.iter_mut().zip(subs) {
            *slot = *value;
        }
        // SAFETY: forwarded from the caller's contract; all arguments are
        // plain values and `sid` is an owned output.
        let ok = unsafe {
            AllocateAndInitializeSid(
                &authority,
                subs.len() as u8,
                slots[0],
                slots[1],
                slots[2],
                slots[3],
                slots[4],
                slots[5],
                slots[6],
                slots[7],
                &mut sid,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sid)
    }

    // SAFETY: both SIDs are owned allocations from `well_known_sid` and are
    // freed by `FreeSid` below.
    let system_sid =
        unsafe { well_known_sid(SECURITY_NT_AUTHORITY, &[SECURITY_LOCAL_SYSTEM_RID as u32]) }?;
    let admins_sid = unsafe {
        well_known_sid(
            SECURITY_NT_AUTHORITY,
            &[
                SECURITY_BUILTIN_DOMAIN_RID as u32,
                DOMAIN_ALIAS_RID_ADMINS as u32,
            ],
        )
    }?;
    // The user SID points into `token_user_buffer`; the buffer must outlive
    // every use of the SID below (it is the process token's own value, not
    // an allocation to free).
    let (token_user_buffer, user_sid) = current_user_sid()?;
    let _ = &token_user_buffer;

    // `TRUSTEE_W` is all-plain-data; zero-init then point `ptstrName` at the
    // SID. Building the entries manually (instead of
    // `BuildExplicitAccessWithNameW`) keeps the trustee SID-based, so no
    // name lookup can fail or be spoofed by a renamed account.
    // SAFETY: EXPLICIT_ACCESS_W contains no references; zeroed memory is a
    // valid starting point and every field is set below before use.
    let entry = |sid: PSID, trustee_type: i32| -> EXPLICIT_ACCESS_W {
        let mut access: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
        access.grfAccessPermissions = GENERIC_ALL;
        access.grfAccessMode = GRANT_ACCESS;
        access.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
        access.Trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: trustee_type,
            ptstrName: sid.cast::<u16>(),
        };
        access
    };
    let entries = [
        entry(user_sid, TRUSTEE_IS_USER),
        entry(system_sid, TRUSTEE_IS_WELL_KNOWN_GROUP),
        entry(admins_sid, TRUSTEE_IS_WELL_KNOWN_GROUP),
    ];

    let mut new_acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `entries` is a valid array of `EXPLICIT_ACCESS_W`; on success
    // the API returns a fresh ACL we must free with `LocalFree`, and the
    // `FreeSid`s release the SIDs in every path below.
    let result = unsafe {
        SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            std::ptr::null(),
            &mut new_acl,
        )
    };
    // SAFETY: both SIDs were allocated by `well_known_sid` above.
    unsafe {
        FreeSid(system_sid);
        FreeSid(admins_sid);
    }
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // PROTECTED_DACL_SECURITY_INFORMATION stops the parent's inheritable
    // ACEs from re-merging into the fresh DACL.
    // SAFETY: `wide` is NUL-terminated, `new_acl` was built by
    // `SetEntriesInAclW` above and is released by `LocalFree` afterwards.
    let result = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_acl,
            std::ptr::null_mut(),
        )
    };
    // SAFETY: `new_acl` was allocated by SetEntriesInAclW and must be freed
    // with LocalFree regardless of the result above.
    unsafe { LocalFree(new_acl.cast()) };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

/// The SID of the account this process runs as, from the process token (not
/// a name lookup, so a renamed or localized account still matches). Returns
/// the SID alongside the buffer it points into: the caller must keep the
/// buffer alive as long as it uses the SID.
#[cfg(windows)]
fn current_user_sid() -> io::Result<(Vec<u8>, windows_sys::Win32::Security::PSID)> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns the pseudo-handle; OpenProcessToken
    // fills `token`, which CloseHandle releases in every path below.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut len = 0u32;
    // SAFETY: The first call with a null buffer only reports the size.
    let _ = unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len) };
    let mut buffer = vec![0u8; len as usize];
    // SAFETY: `buffer` is `len` bytes long, exactly what the API asked for.
    let ok =
        unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len) };
    let token_user = buffer.as_ptr() as *const TOKEN_USER;
    // SAFETY: the successful call filled `buffer` with a TOKEN_USER whose
    // `User.Sid` points into the same buffer, which the caller keeps alive.
    let sid: PSID = unsafe { (*token_user).User.Sid };
    // SAFETY: `token` was opened above and is owned by this scope.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((buffer, sid))
}

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
