//! Filesystem hardening of the paths the server owns (config dir +
//! `acts.toml`, log dir, store dir) — the public surface
//! `acts_server::{ensure_dir_secure, ensure_file_secure, validate_dir_secure,
//! validate_file_secure, validate_startup_dirs}`.
//!
//! Unix cases need real mode bits and symlinks (`std::os::unix::fs::symlink`),
//! so they are `#[cfg(unix)]` and run in CI. The windows cases exercise the
//! DACL path with the actual security API and run wherever the server is
//! built for windows.

use acts_server::{ensure_dir_secure, ensure_file_secure, validate_dir_secure};
#[cfg(unix)]
use acts_server::{validate_file_secure, validate_startup_dirs};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use acts_server::Config;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Scratch dir for one case, wiped so it starts empty.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("acts-fs-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The unix mode of `path`.
#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn ensure_dir_secure_yields_0700_and_is_idempotent() {
    let dir = scratch("dir-mode");
    let target = dir.join("log");
    ensure_dir_secure(&target).unwrap();
    assert_eq!(mode_of(&target), 0o700);
    // A second pass over the same directory must not fail (and keeps the
    // mode).
    ensure_dir_secure(&target).unwrap();
    assert_eq!(mode_of(&target), 0o700);
    validate_dir_secure(&target, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn loose_store_dir_is_flagged_and_fixed_by_the_lenient_pass() {
    let dir = scratch("loose-dir");
    let store = dir.join("store");
    fs::create_dir_all(&store).unwrap();
    fs::set_permissions(&store, fs::Permissions::from_mode(0o777)).unwrap();

    // strict: refuse.
    let err = validate_dir_secure(&store, true).unwrap_err();
    assert!(err.to_string().contains(store.to_string_lossy().as_ref()));

    // lenient: warn and fix, so the next validation passes.
    validate_dir_secure(&store, false).unwrap();
    assert_eq!(mode_of(&store), 0o700);
    validate_dir_secure(&store, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn symlinked_dir_is_rejected_in_both_modes() {
    let dir = scratch("symlink-dir");
    let real = dir.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = dir.join("log");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let strict = validate_dir_secure(&link, true).unwrap_err();
    assert!(strict.to_string().contains("symlink"));
    let lenient = validate_dir_secure(&link, false).unwrap_err();
    assert!(lenient.to_string().contains("symlink"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn symlinked_intermediate_component_is_rejected() {
    let dir = scratch("symlink-mid");
    let real = dir.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let path = link.join("store");

    let err = validate_dir_secure(&path, true).unwrap_err();
    assert!(err.to_string().contains("symlink"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn world_writable_parent_without_sticky_is_flagged() {
    let dir = scratch("ww-parent");
    let parent = dir.join("parent");
    let child = parent.join("data");
    fs::create_dir_all(&child).unwrap();
    // Pin the child's own mode so the strict passes below test the parent
    // handling regardless of the runner's umask.
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();

    let err = validate_dir_secure(&child, true).unwrap_err();
    assert!(err.to_string().contains("world-writable"));

    // The lenient pass removes the other-write bit from the parent (keeping
    // the rest of the mode), and then everything validates.
    validate_dir_secure(&child, false).unwrap();
    assert_eq!(mode_of(&parent), 0o775);
    validate_dir_secure(&child, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn world_writable_parent_with_sticky_bit_passes() {
    let dir = scratch("sticky-parent");
    let parent = dir.join("parent");
    let child = parent.join("data");
    fs::create_dir_all(&child).unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o1777)).unwrap();
    validate_dir_secure(&child, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn ensure_file_secure_creates_0600_without_a_window() {
    let dir = scratch("file-mode");
    let file = dir.join("acts.toml");
    ensure_file_secure(&file, "cache_cap = 1024\n").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "cache_cap = 1024\n");
    assert_eq!(mode_of(&file), 0o600);
    // Idempotent: the existing file is kept, its contents untouched.
    ensure_file_secure(&file, "other = true\n").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "cache_cap = 1024\n");
    validate_file_secure(&file, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn loose_file_is_flagged_and_fixed_by_the_lenient_pass() {
    let dir = scratch("loose-file");
    let file = dir.join("acts.toml");
    fs::write(&file, "a = 1\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

    let err = validate_file_secure(&file, true).unwrap_err();
    assert!(
        err.to_string()
            .contains(&file.to_string_lossy().to_string())
    );

    validate_file_secure(&file, false).unwrap();
    assert_eq!(mode_of(&file), 0o600);
    validate_file_secure(&file, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn symlinked_file_is_rejected_and_never_written_through() {
    let dir = scratch("symlink-file");
    let victim = dir.join("victim.txt");
    fs::write(&victim, "keep\n").unwrap();
    let link = dir.join("acts.toml");
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    // TOCTOU-style final component: the create path must refuse instead of
    // following the link.
    let err = ensure_file_secure(&link, "swap\n").unwrap_err();
    assert!(err.to_string().contains("symlink"));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep\n");
    assert_eq!(fs::read_to_string(&link).unwrap(), "keep\n");

    let strict = validate_file_secure(&link, true).unwrap_err();
    assert!(strict.to_string().contains("symlink"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn ensure_file_secure_refuses_a_broken_symlink() {
    let dir = scratch("broken-link");
    let link = dir.join("acts.toml");
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.join("missing").join("target"), &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(dir.join("missing").join("target"), &link).unwrap();
    let err = ensure_file_secure(&link, "x\n").unwrap_err();
    assert!(err.to_string().contains("symlink"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn startup_pass_refuses_a_symlinked_config_dir_when_strict() {
    let dir = scratch("startup-strict");
    let real = dir.join("real-config");
    fs::create_dir_all(&real).unwrap();
    let link = dir.join("config");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    ensure_file_secure(&link.join("acts.toml"), "").unwrap();
    let config = Config::create(&link.join("acts.toml")).unwrap();

    let err = validate_startup_dirs(&link, &config, true).unwrap_err();
    assert!(err.to_string().contains("symlink"), "got: {err}");

    // Lenient mode refuses too: the symlink itself is not fixable.
    assert!(validate_startup_dirs(&link, &config, false).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn startup_pass_accepts_a_hardened_tree() {
    use acts_server::ensure_default_config;

    let dir = scratch("startup-ok");
    ensure_default_config(&dir).unwrap();
    let config = Config::create(&dir.join("acts.toml")).unwrap();
    validate_startup_dirs(&dir, &config, true).unwrap();
    validate_startup_dirs(&dir, &config, false).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

/// The embedded default config stays valid TOML after the marker is
/// replaced, and the hardening sections carry the documented keys (the
/// plugin crates own their deserialization; this only checks the template
/// itself, via the shipped `ensure_default_config` path).
#[test]
fn default_config_template_parses_and_carries_the_hardening_sections() {
    use acts_server::{config_path, ensure_default_config};

    let dir = scratch("template");
    ensure_default_config(&dir).unwrap();
    let text = fs::read_to_string(config_path(&dir)).unwrap();
    let table: toml::Table = toml::from_str(&text).expect("default acts.toml must parse");
    let spot = |key: &str| {
        table
            .get(key)
            .unwrap_or_else(|| panic!("missing section {key}"))
    };
    spot("ingress");
    assert_eq!(
        spot("ingress").get("global_inflight"),
        Some(&toml::Value::Integer(1024))
    );
    assert_eq!(
        spot("ingress").get("parse_inflight"),
        Some(&toml::Value::Integer(128))
    );
    assert!(spot("subscriptions").get("global_max").is_some());
    assert!(spot("subscriptions").get("create_rate_per_sec").is_some());
    assert_eq!(
        spot("shutdown").get("drain_timeout_ms"),
        Some(&toml::Value::Integer(5000))
    );
    // Optional transports ship commented out — a fresh install must not
    // open a port or reach for a broker until the operator uncomments.
    for optional in ["[web]", "[grpc]", "[nats]"] {
        assert!(
            text.contains(&format!("# {optional}")),
            "the template must document {optional} (commented out)"
        );
    }
    spot("server");
    assert_eq!(
        spot("server").get("fs_strict"),
        Some(&toml::Value::Boolean(false))
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The explicit `fs_strict` key wins; without it the posture level decides
/// (strict → true, everything else → false, including via
/// `[security].level` and the section's own `level`).
#[test]
fn fs_strict_default_follows_the_level_and_explicit_key_wins() {
    use acts_server::{Config, fs_strict};

    let load = |name: &str, body: &str| {
        let dir = scratch(name);
        let path = dir.join("acts.toml");
        fs::write(&path, body).unwrap();
        let config = Config::create(&path).unwrap();
        let _ = fs::remove_dir_all(&dir);
        config
    };

    // no [server], standard default
    assert!(!fs_strict(&load("level-none", "")).unwrap());
    // [security].level drives the default
    assert!(fs_strict(&load("level-security", "[security]\nlevel = \"strict\"\n")).unwrap());
    assert!(
        !fs_strict(&load(
            "level-security-standard",
            "[security]\nlevel = \"standard\"\n"
        ))
        .unwrap()
    );
    assert!(
        !fs_strict(&load(
            "level-security-permissive",
            "[security]\nlevel = \"permissive\"\n"
        ))
        .unwrap()
    );
    // the [server] section's own level beats [security]
    assert!(
        fs_strict(&load(
            "level-server",
            "[security]\nlevel = \"standard\"\n[server]\nlevel = \"strict\"\n"
        ))
        .unwrap()
    );
    // an explicit key wins over the level
    assert!(
        !fs_strict(&load(
            "level-explicit-off",
            "[security]\nlevel = \"strict\"\n[server]\nfs_strict = false\n"
        ))
        .unwrap()
    );
    assert!(fs_strict(&load("level-explicit-on", "[server]\nfs_strict = true\n")).unwrap());
}

#[cfg(windows)]
mod windows_dacl {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, AllocateAndInitializeSid,
        DACL_SECURITY_INFORMATION, EqualSid, FreeSid, GetAce, GetAclInformation, PSID,
        SECURITY_NT_AUTHORITY, SECURITY_WORLD_SID_AUTHORITY, SID_IDENTIFIER_AUTHORITY,
    };
    use windows_sys::Win32::System::SystemServices::{
        DOMAIN_ALIAS_RID_USERS, SECURITY_BUILTIN_DOMAIN_RID, SECURITY_WORLD_RID,
    };

    /// Whether the DACL of `path` grants anything to the SID built from
    /// `subs` under `authority` (Everyone is world+[0], Users is
    /// NT+[32,545], …). A SID's identity is its leading `subs.len()`
    /// sub-authorities.
    fn grants_to(path: &Path, authority: SID_IDENTIFIER_AUTHORITY, subs: &[u32]) -> bool {
        let mut sid: PSID = std::ptr::null_mut();
        let mut slots = [0u32; 8];
        for (slot, value) in slots.iter_mut().zip(subs) {
            *slot = *value;
        }
        // SAFETY: plain-value arguments; the SID is freed with FreeSid.
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
        assert_ne!(ok, 0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd: *mut c_void = std::ptr::null_mut();
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated; `sd` must be freed with
        // LocalFree and carries the lifetime of `dacl`.
        let result = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        assert_eq!(result, ERROR_SUCCESS);
        let mut size: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `dacl` is a live ACL from the call above.
        let ok = unsafe {
            GetAclInformation(
                dacl,
                (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        };
        assert_ne!(ok, 0);
        let mut found = false;
        for index in 0..size.AceCount {
            let mut ace_ptr: *mut c_void = std::ptr::null_mut();
            // SAFETY: `index` is within AceCount.
            let ok = unsafe { GetAce(dacl, index, &mut ace_ptr) };
            assert_ne!(ok, 0);
            // The SID follows the ACE header and its DWORD access mask;
            // GetAce returned an ACE of at least that size.
            let sid_ptr = unsafe {
                ace_ptr
                    .cast::<u8>()
                    .add(std::mem::size_of::<ACE_HEADER>() + 4)
            } as PSID;
            // SAFETY: both SIDs are live and well-formed.
            // SAFETY: both SIDs are live and well-formed.
            if unsafe { EqualSid(sid_ptr, sid) } != 0 {
                found = true;
            }
        }
        // SAFETY: both were allocated by the security API above.
        unsafe {
            LocalFree(sd);
            FreeSid(sid);
        }
        found
    }

    fn everyone() -> SID_IDENTIFIER_AUTHORITY {
        SID_IDENTIFIER_AUTHORITY {
            Value: SECURITY_WORLD_SID_AUTHORITY.Value,
        }
    }

    #[test]
    fn dir_dacl_excludes_everyone_and_users_and_is_idempotent() {
        let dir = scratch("win-dacl");
        let target = dir.join("log");
        ensure_dir_secure(&target).unwrap();
        ensure_dir_secure(&target).unwrap();

        assert!(!grants_to(
            &target,
            everyone(),
            &[SECURITY_WORLD_RID as u32]
        ));
        assert!(!grants_to(
            &target,
            SID_IDENTIFIER_AUTHORITY {
                Value: SECURITY_NT_AUTHORITY.Value,
            },
            &[
                SECURITY_BUILTIN_DOMAIN_RID as u32,
                DOMAIN_ALIAS_RID_USERS as u32,
            ]
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_dacl_is_restricted_too() {
        let dir = scratch("win-file");
        let file = dir.join("acts.toml");
        ensure_file_secure(&file, "cache_cap = 1024\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "cache_cap = 1024\n");
        assert!(!grants_to(&file, everyone(), &[SECURITY_WORLD_RID as u32]));

        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
#[test]
fn validate_dir_secure_passes_on_a_hardened_dir() {
    let dir = scratch("win-validate");
    let target = dir.join("store");
    ensure_dir_secure(&target).unwrap();
    validate_dir_secure(&target, true).unwrap();
    let _ = fs::remove_dir_all(&dir);
}
