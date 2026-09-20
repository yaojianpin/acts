//! The exclusive database lease: one live instance per database, enforced on
//! every write with a fencing token.
//!
//! The engine's mutual exclusion is otherwise process-local — the document
//! locks (the store's `collection` module), the PID write lanes, the scheduler
//! lanes and the in-memory claim registries all end at the process boundary,
//! and the durable rows carry no owner. Two processes over one database
//! therefore both recover the same rows, both schedule the same processes and
//! both compute their index-row deltas from the same read, and the backend
//! cannot tell them apart: a batch is atomic, the read the batch was computed
//! from is not.
//!
//! [`DbLease`] closes that with the one primitive a shared database can offer:
//! a compare-and-swap. The lease is a single row ([`LEASE_KEY`]) holding a
//! [`LeaseRecord`] — the holder, a monotonically increasing fence and an
//! expiry — and every transition of it (acquire, renew, release) is one
//! [`KvStore::batch`] with that row as its guard, so exactly one contender can
//! win each transition no matter how many processes race it:
//!
//! - **Acquire** applies only while the row is still absent, or still holds
//!   the exact expired record the contender read. The winner's fence is one
//!   above the record it replaced, so a fence is unique per acquisition for
//!   the lifetime of the database and grows across crashes and restarts.
//! - **Renew** applies only while the row still holds the winner's own bytes.
//!   A holder that was taken over (its expiry passed while it was stalled, and
//!   someone else acquired) gets [`ActError::LeaseLost`] instead of a silent
//!   extension.
//! - **Release** replaces the row with a vacant record only while it still
//!   holds the holder's bytes, so a departing instance can never touch its
//!   successor's lease — and the fence it leaves behind keeps the sequence
//!   going.
//!
//! Enforcement is [`FencedStore`]: a [`KvStore`] view whose `put`/`delete`/
//! `batch` commit under the holder's lease row as an extra guard. A write from
//! an instance whose fence is stale is refused with [`ActError::LeaseLost`]
//! *inside the same atomic unit that would have applied it*, so it cannot
//! commit — not even in the window between "my lease expired" and "I noticed".
//! Wrapping the engine's store in it puts that fence on every durable mutation
//! the engine makes: PID-lane flushes, scheduler gates, recovery claims,
//! task/vars/proc rows, deployments and deliveries.
//!
//! [`LeaseKeeper`] runs the renewal loop the expiry needs, and stops the
//! engine when the lease is lost — whether a renewal noticed it or a write did:
//! the lease is marked lost (so every write is refused immediately) and the
//! shutdown token is cancelled, which is how the scheduler's loops, the store
//! writer and the timers stop admitting work.
//!
//! What fencing does NOT do is undo an effect an instance had already started
//! — an act's outbound request, say — before it noticed the loss; it
//! guarantees the *database* has a single writer, and the engine's
//! at-least-once act semantics are unchanged. Acquisitions and takeovers also
//! compare wall-clock deadlines, so the instances sharing a database must
//! agree on the clock.

use super::{KvStore, ScanOptions, StoreBatchOp, StoreGuard};
use crate::{ActError, Result};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// The key the lease row lives under.
///
/// It cannot collide with a collection: every collection key starts with its
/// [`crate::store::StoreIden`] prefix, and every collection scan is bounded by
/// `<prefix>-id-`/`<prefix>-<field>-`. The name stays inside the charset the
/// NATS KV backend accepts (`[-/_=.a-zA-Z0-9]`) so the same database holds the
/// same row whatever backend is in front of it.
pub const LEASE_KEY: &str = "__acts_lease__";

/// Default time an unrenewed lease stays valid.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(30);

/// Default renewal interval — a third of the default TTL, so two consecutive
/// renewals can be lost before the lease expires.
pub const DEFAULT_LEASE_RENEW: Duration = Duration::from_secs(10);

/// How many times an acquisition re-reads and re-tries after losing the
/// compare-and-swap race. Each retry is a fresh read of a row that changed
/// under it, so a contender that keeps losing gives up rather than spinning.
const ACQUIRE_ATTEMPTS: usize = 8;

/// The lease row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    /// Who holds it — the instance id the deployment configured (default
    /// `<host>:<pid>`). Diagnostics only: authority comes from the fence.
    pub owner: String,
    /// Monotonic fencing token: strictly greater than every fence the
    /// database has held before, for every acquisition, including takeovers
    /// after a crash. A write carrying a fence below the stored one is a stale
    /// holder's.
    pub fence: u64,
    /// Wall-clock millisecond deadline after which the lease may be taken
    /// over. [`i64::MAX`] when the caller asked for no expiry.
    pub expires_at: i64,
}

impl LeaseRecord {
    /// Whether the record's deadline has passed at `now_millis`.
    pub fn is_expired_at(&self, now_millis: i64) -> bool {
        now_millis >= self.expires_at
    }

    /// The row a release leaves behind: no owner, expired, and the fence the
    /// releasing holder had.
    ///
    /// A release MUST NOT delete the row — the next acquisition could then
    /// start the fence sequence over at 1, and a write from the released
    /// instance (whose fence is 1) would be accepted as the new holder's. The
    /// vacant row keeps the sequence going: the next acquirer takes it over
    /// and bumps the fence, exactly as it would for an expired holder.
    fn vacant(fence: u64) -> Self {
        Self {
            owner: String::new(),
            fence,
            expires_at: 0,
        }
    }

    /// Whether nobody holds the lease (a released row). A vacant row is
    /// expired by construction, so any contender takes it over.
    pub fn is_vacant(&self) -> bool {
        self.owner.is_empty()
    }

    /// A diagnostic one-liner about the holder, for a startup error or a log
    /// line: the instance id, the fence and the remaining validity.
    ///
    /// Safe to surface: the lease row holds an instance id the deployment set,
    /// a counter and a deadline — never a token or a credential.
    pub fn describe(&self) -> String {
        if self.is_vacant() {
            return format!("released (fence={})", self.fence);
        }
        let remaining = self
            .expires_at
            .saturating_sub(crate::utils::time::time_millis());
        format!(
            "owner={} fence={} expires_in={}s",
            self.owner,
            self.fence,
            remaining.div_euclid(1000)
        )
    }
}

/// State of the held lease: the record this instance wrote and the exact bytes
/// it wrote it as (the bytes every fenced write re-checks).
#[derive(Debug)]
struct HeldRow {
    record: LeaseRecord,
    bytes: Vec<u8>,
}

struct LeaseInner {
    /// The UNFENCED store: the lease's own transitions must not be gated by
    /// the lease they implement.
    kv: Arc<dyn KvStore>,
    owner: String,
    ttl_millis: i64,
    held: RwLock<Option<HeldRow>>,
    /// Set once the lease is known to be gone: every fenced write is refused
    /// from then on, without touching the store.
    lost: AtomicBool,
    /// The engine's shutdown token, registered by [`LeaseKeeper::start`]: a
    /// lost lease stops the engine whichever way the loss was noticed — a
    /// renewal that was refused, or a write that found its fence stale.
    shutdown: Mutex<Option<CancellationToken>>,
}

impl LeaseInner {
    /// The exact row bytes this instance last wrote, or `None` when it holds
    /// nothing (released or lost).
    fn held_bytes(&self) -> Option<Vec<u8>> {
        self.held.read().as_ref().map(|row| row.bytes.clone())
    }

    fn guard(&self) -> Result<StoreGuard> {
        match self.held_bytes() {
            Some(bytes) => Ok(StoreGuard::holds(LEASE_KEY, bytes)),
            // Every fenced write checks `is_lost` first and the flag is set
            // wherever this becomes `None`, so reaching here means the two
            // raced; refusing is the safe side of that race.
            None => Err(ActError::LeaseLost),
        }
    }

    /// Give the lease up for good: refuse every further write, and stop the
    /// engine that was running on it.
    fn mark_lost(&self) {
        self.lost.store(true, Ordering::SeqCst);
        *self.held.write() = None;
        let shutdown = self.shutdown.lock();
        if let Some(token) = shutdown.as_ref() {
            token.cancel();
        }
    }

    fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// Hand the lease the engine whose shutdown it must trigger.
    fn register_shutdown(&self, token: &CancellationToken) {
        *self.shutdown.lock() = Some(token.clone());
        if self.is_lost() {
            token.cancel();
        }
    }

    /// Store the record this instance just committed as the row's bytes.
    fn install(&self, record: LeaseRecord, bytes: Vec<u8>) {
        self.lost.store(false, Ordering::SeqCst);
        *self.held.write() = Some(HeldRow { record, bytes });
    }
}

/// An exclusive lease on one database, held by one instance.
///
/// Acquire it with [`DbLease::acquire`] before starting the engine, wrap the
/// store in [`DbLease::fenced`] for the engine, and renew it with
/// [`LeaseKeeper`]. Dropping the handle does not release the lease — a crash
/// must leave the row to expire — so a graceful stop calls
/// [`DbLease::release`] (the keeper does it for its owner).
pub struct DbLease {
    inner: Arc<LeaseInner>,
}

impl std::fmt::Debug for DbLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbLease")
            .field("owner", &self.inner.owner)
            .field("fence", &self.fence())
            .field("expires_at", &self.expires_at())
            .field("lost", &self.inner.is_lost())
            .finish()
    }
}

impl DbLease {
    /// Take the database's lease for `owner` for `ttl`, or report who holds it
    /// when a live lease is already there.
    ///
    /// `Ok(Some(lease))` means this instance is the single writer and every
    /// lease it wins has a fence above every previous holder's.
    /// `Ok(None)` means a live lease is held by someone else — the caller must
    /// not run an engine, but may retry later (a crashed holder's lease
    /// expires after its TTL).
    ///
    /// Fails when the backend cannot commit a guarded batch — a [`KvStore`]
    /// whose `batch` refuses guards — because an exclusive lease without a
    /// conditional commit is exactly the non-atomic pre-check it exists to
    /// replace.
    pub async fn acquire(
        store: Arc<dyn KvStore>,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<Self>> {
        Self::acquire_at(store, owner, ttl, crate::utils::time::time_millis()).await
    }

    /// [`DbLease::acquire`] against an explicit clock reading: expiry is the
    /// only input the lease takes from the environment, so tests drive
    /// takeovers deterministically instead of sleeping.
    pub async fn acquire_at(
        store: Arc<dyn KvStore>,
        owner: &str,
        ttl: Duration,
        now_millis: i64,
    ) -> Result<Option<Self>> {
        let ttl_millis = ttl_to_millis(ttl);
        let inner = Arc::new(LeaseInner {
            kv: store,
            owner: owner.to_string(),
            ttl_millis,
            held: RwLock::new(None),
            lost: AtomicBool::new(false),
            shutdown: Mutex::new(None),
        });

        for _ in 0..ACQUIRE_ATTEMPTS {
            let stored = inner.kv.one(LEASE_KEY).await?;
            let (guard, fence) = match stored.as_deref().map(parse_record) {
                // Nothing there: the fence starts the database's sequence.
                None => (StoreGuard::absent(LEASE_KEY), 1),
                Some(Ok(current)) if current.is_expired_at(now_millis) => (
                    // The dead holder's exact bytes are the guard, so the
                    // takeover applies only if the row is still that record.
                    StoreGuard::holds(LEASE_KEY, stored.clone().unwrap_or_default()),
                    current.fence.checked_add(1).ok_or_else(|| {
                        ActError::Store(
                            "database lease fence overflow: the lease row is corrupt".to_string(),
                        )
                    })?,
                ),
                // A live lease, or an unreadable row. An unreadable row is
                // treated as held: refusing to start is recoverable, adopting
                // a foreign row is not.
                Some(Ok(_)) => return Ok(None),
                Some(Err(err)) => {
                    warn!(error = %err, "unreadable database lease row; treating it as held");
                    return Ok(None);
                }
            };

            let record = LeaseRecord {
                owner: owner.to_string(),
                fence,
                expires_at: expiry_from(now_millis, ttl_millis),
            };
            // The record this contender proposes. A takeover replaces another
            // holder's row and MUST bump the fence; the state is installed
            // only after the guarded batch applied it.
            let bytes = serde_json::to_vec(&record).map_err(map_serde_err)?;
            let ops = [StoreBatchOp::Put {
                key: LEASE_KEY.to_string(),
                value: bytes.clone(),
            }];
            if inner.kv.batch(&ops, &[guard]).await? {
                inner.install(record, bytes);
                info!(owner = %owner, fence, "database lease acquired");
                return Ok(Some(Self { inner }));
            }
            // Another contender won the same transition: re-read and decide
            // again from what it committed.
            continue;
        }
        Err(ActError::Store(
            "could not acquire the database lease: the lease row kept changing under it \
             (another instance is restarting repeatedly)"
                .to_string(),
        ))
    }

    /// The instance id this lease was taken for.
    pub fn owner(&self) -> &str {
        &self.inner.owner
    }

    /// The fencing token: strictly increasing per acquisition of this
    /// database, and the value every write of this instance is fenced with.
    pub fn fence(&self) -> u64 {
        self.inner
            .held
            .read()
            .as_ref()
            .map(|row| row.record.fence)
            .unwrap_or(0)
    }

    /// The wall-clock deadline of the current lease, or `0` when nothing is
    /// held any more.
    pub fn expires_at(&self) -> i64 {
        self.inner
            .held
            .read()
            .as_ref()
            .map(|row| row.record.expires_at)
            .unwrap_or(0)
    }

    /// Whether this instance has been taken over (or released) and every
    /// further write is refused.
    pub fn is_lost(&self) -> bool {
        self.inner.is_lost()
    }

    /// Extend the lease by `ttl` from now, keeping the fence.
    ///
    /// [`ActError::LeaseLost`] means the row no longer holds this instance's
    /// bytes — it was taken over after the lease expired — and the lease is
    /// marked lost so no further write is attempted. A store failure is
    /// returned as-is and does NOT lose the lease: the row still holds this
    /// instance's bytes, so a later renewal that applies still proves
    /// ownership, and the row only changes hands through a guarded takeover
    /// that would make this call fail.
    pub async fn renew(&self) -> Result<()> {
        self.renew_at(crate::utils::time::time_millis()).await
    }

    /// [`DbLease::renew`] against an explicit clock reading.
    pub async fn renew_at(&self, now_millis: i64) -> Result<()> {
        if self.inner.is_lost() {
            return Err(ActError::LeaseLost);
        }
        let (guard, record, bytes) = {
            let held = self.inner.held.read();
            let Some(row) = held.as_ref() else {
                return Err(ActError::LeaseLost);
            };
            let record = LeaseRecord {
                owner: row.record.owner.clone(),
                fence: row.record.fence,
                expires_at: expiry_from(now_millis, self.inner.ttl_millis),
            };
            let bytes = serde_json::to_vec(&record).map_err(map_serde_err)?;
            (
                StoreGuard::holds(LEASE_KEY, row.bytes.clone()),
                record,
                bytes,
            )
        };
        let ops = [StoreBatchOp::Put {
            key: LEASE_KEY.to_string(),
            value: bytes.clone(),
        }];
        if self.inner.kv.batch(&ops, &[guard]).await? {
            *self.inner.held.write() = Some(HeldRow { record, bytes });
            return Ok(());
        }
        // The row is not ours any more: another instance holds the lease and
        // this one must stop writing.
        self.inner.mark_lost();
        error!(
            owner = %self.inner.owner,
            fence = self.fence(),
            "database lease taken over: this instance stops writing"
        );
        Err(ActError::LeaseLost)
    }

    /// Whether the current lease's deadline has passed at `now_millis`.
    pub fn is_expired_at(&self, now_millis: i64) -> bool {
        self.inner
            .held
            .read()
            .as_ref()
            .is_some_and(|row| row.record.is_expired_at(now_millis))
    }

    /// Whether the lease is dead: taken over, released, or past its deadline.
    pub fn is_expired(&self) -> bool {
        self.is_lost() || self.is_expired_at(crate::utils::time::time_millis())
    }

    /// Give the lease up, so the next instance starts immediately instead of
    /// waiting for the TTL.
    ///
    /// The row is replaced by a vacant record — not deleted — while it still
    /// holds this instance's bytes: a successor's lease is never touched, and
    /// the fence sequence continues across the release, so a write from this
    /// instance (§fence) can never be accepted as the next holder's. After
    /// this, every write through [`DbLease::fenced`] is refused.
    pub async fn release(&self) -> Result<()> {
        let Some(bytes) = self.inner.held_bytes() else {
            self.inner.mark_lost();
            return Ok(());
        };
        let fence = self.fence();
        let guard = StoreGuard::holds(LEASE_KEY, bytes);
        let vacant = LeaseRecord::vacant(fence);
        let vacancy = serde_json::to_vec(&vacant).map_err(map_serde_err)?;
        let outcome = self
            .inner
            .kv
            .batch(
                &[StoreBatchOp::Put {
                    key: LEASE_KEY.to_string(),
                    value: vacancy,
                }],
                &[guard],
            )
            .await;
        // Either way this instance is done: a release no guard admits means
        // the row already belongs to someone else (or is gone), which is the
        // same outcome for this holder.
        self.inner.mark_lost();
        match outcome {
            Ok(true) => {
                info!(owner = %self.inner.owner, fence, "database lease released");
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// The store the engine must run on: reads pass through, every write is
    /// committed only while this lease still holds the row.
    pub fn fenced(&self) -> Arc<dyn KvStore> {
        Arc::new(FencedStore {
            inner: self.inner.kv.clone(),
            lease: self.inner.clone(),
        })
    }
}

/// A [`KvStore`] view that refuses to write for an instance whose lease is
/// gone.
///
/// Reads delegate untouched — a stale instance's reads cannot damage the
/// holder's rows, and refusing them would only replace one confusing failure
/// with another. Writes go through [`KvStore::batch`] with the lease
/// row as the guard: the check and the write are one atomic unit on the
/// backend, so a taken-over instance's write either commits while its fence is
/// still the row's, or is reported as [`ActError::LeaseLost`] and does not
/// land.
pub struct FencedStore {
    inner: Arc<dyn KvStore>,
    lease: Arc<LeaseInner>,
}

impl FencedStore {
    /// The guard every write of this instance carries.
    fn lease_guard(&self) -> Result<StoreGuard> {
        self.lease.guard()
    }

    /// Commit `ops` under the lease fence.
    async fn guarded_write(&self, ops: Vec<StoreBatchOp>) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        if self.lease.is_lost() {
            return Err(ActError::LeaseLost);
        }
        let guard = self.lease_guard()?;
        if self.inner.batch(&ops, &[guard]).await? {
            return Ok(());
        }
        // The only guard of this batch is the lease row, so a refused batch
        // means exactly one thing: this instance no longer holds it.
        self.lease.mark_lost();
        error!(
            owner = %self.lease.owner,
            "database lease no longer held: refusing to write"
        );
        Err(ActError::LeaseLost)
    }
}

#[async_trait::async_trait]
impl KvStore for FencedStore {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        self.inner.many(keys).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.guarded_write(vec![StoreBatchOp::Put {
            key: key.to_string(),
            value,
        }])
        .await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.guarded_write(vec![StoreBatchOp::Delete {
            key: key.to_string(),
        }])
        .await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if guards.is_empty() {
            // Only the lease guards the batch, so the outcome is exactly the
            // write's: applied, or refused as a lost lease.
            return self.guarded_write(ops.to_vec()).await.map(|()| true);
        }
        if self.lease.is_lost() {
            return Err(ActError::LeaseLost);
        }
        let mut all = Vec::with_capacity(guards.len() + 1);
        all.push(self.lease_guard()?);
        all.extend(guards.iter().cloned());
        if self.inner.batch(ops, &all).await? {
            return Ok(true);
        }
        // Either the caller's guard or the lease guard failed. Tell them apart
        // with one read: only a lease that is no longer this instance's makes
        // the loss permanent.
        if self.lease_row_is_ours().await? {
            Ok(false)
        } else {
            self.lease.mark_lost();
            error!(
                owner = %self.lease.owner,
                "database lease no longer held: refusing to write"
            );
            Err(ActError::LeaseLost)
        }
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

impl FencedStore {
    /// Whether the lease row still holds this instance's bytes. Only read on
    /// the conflict path, to separate "your guard lost" from "your lease
    /// lost".
    async fn lease_row_is_ours(&self) -> Result<bool> {
        let expected = self.lease.held_bytes();
        let stored = self.inner.one(LEASE_KEY).await?;
        Ok(expected.is_some() && expected.as_deref() == stored.as_deref())
    }
}

/// Keeps a lease alive: renews it every `interval` and stops the engine when
/// it cannot.
///
/// The task owns the renewal loop the lease's expiry needs. Two things end it:
///
/// - `shutdown` fires (the embedder is closing): the lease is released, so a
///   rolling restart takes over at once instead of waiting out the TTL;
/// - a renewal fails because the lease was taken over: the lease is marked
///   lost — every further write through [`DbLease::fenced`] is refused with
///   [`ActError::LeaseLost`] — and `shutdown` is cancelled, which stops the
///   scheduler's lanes, the store writer and the timers.
///
/// [`LeaseKeeper::start`] also registers `shutdown` on the lease, so a loss
/// noticed by a *write* (its fence no longer matches the row) stops the engine
/// the same way instead of only refusing that one write.
///
/// A renewal that fails for another reason (the store is unreachable, say) is
/// retried at the next interval: the row still holds this instance's bytes, so
/// the lease is still this instance's until a takeover replaces them — which
/// the next successful renewal would prove. If the deadline does pass while
/// the store stays down, the lease is marked lost and the engine stopped,
/// because another instance may hold the database from that moment on.
pub struct LeaseKeeper {
    handle: tokio::task::JoinHandle<()>,
    lease: Arc<DbLease>,
    shutdown: CancellationToken,
}

impl LeaseKeeper {
    /// Start renewing `lease` every `interval` until `shutdown` fires or the
    /// lease is lost.
    ///
    /// The task ends by itself in both cases; [`LeaseKeeper::stop`] is how a
    /// caller waits for it (and therefore for the release on the graceful
    /// path) instead of leaving it detached.
    pub fn start(lease: Arc<DbLease>, interval: Duration, shutdown: CancellationToken) -> Self {
        // The lease owns the engine's shutdown from here: a loss noticed by a
        // write refuses writes and stops the engine just as a failed renewal
        // does.
        lease.inner.register_shutdown(&shutdown);
        let inner = lease.inner.clone();
        let token = shutdown.clone();
        let renewing = lease.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        // Graceful stop: hand the lease back so the successor
                        // does not wait for the TTL (and takes a higher fence).
                        if let Err(err) = renewing.release().await {
                            warn!(error = %err, "failed to release the database lease");
                        }
                        return;
                    }
                    _ = tokio::time::sleep(interval) => {}
                }
                match renewing.renew().await {
                    Ok(()) => {}
                    Err(ActError::LeaseLost) => {
                        // `renew` already marked the lease lost and logged it;
                        // stopping the engine is what makes the loss visible.
                        token.cancel();
                        return;
                    }
                    Err(err) => {
                        if renewing.is_expired() {
                            error!(
                                error = %err,
                                owner = %inner.owner,
                                "database lease expired while renewal failed: this instance stops writing"
                            );
                            inner.mark_lost();
                            token.cancel();
                            return;
                        }
                        warn!(
                            error = %err,
                            "database lease renewal failed; retrying before the lease expires"
                        );
                    }
                }
            }
        });
        Self {
            handle,
            lease,
            shutdown,
        }
    }

    /// The lease this keeper maintains.
    pub fn lease(&self) -> &Arc<DbLease> {
        &self.lease
    }

    /// Stop the keeper: cancels the shutdown token (the graceful path) and
    /// waits for the task, so the lease has been released when this returns.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.handle.await;
    }
}

fn parse_record(bytes: &[u8]) -> Result<LeaseRecord> {
    serde_json::from_slice(bytes).map_err(|err| {
        ActError::Store(format!(
            "unreadable database lease row ({} bytes): {err}",
            bytes.len()
        ))
    })
}

fn expiry_from(now_millis: i64, ttl_millis: i64) -> i64 {
    now_millis.saturating_add(ttl_millis)
}

/// A TTL of zero means no expiry at all: the lease is released or taken over
/// only on an explicit release, never by the clock.
fn ttl_to_millis(ttl: Duration) -> i64 {
    if ttl.is_zero() {
        return i64::MAX;
    }
    i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX)
}

fn map_serde_err(err: serde_json::Error) -> ActError {
    ActError::Store(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, StoreBatchOp};

    async fn store() -> Arc<dyn KvStore> {
        Arc::new(MemoryStore::new())
    }

    async fn raw(store: &Arc<dyn KvStore>, key: &str) -> Option<Vec<u8>> {
        store.one(key).await.unwrap()
    }

    async fn acquire(store: &Arc<dyn KvStore>, owner: &str, now: i64) -> Option<DbLease> {
        DbLease::acquire_at(store.clone(), owner, Duration::from_secs(10), now)
            .await
            .unwrap()
    }

    /// The lease is exclusive: while one instance holds a live lease, another
    /// cannot take it — and the refusal is a plain `None`, not an error.
    #[tokio::test]
    async fn a_live_lease_is_exclusive() {
        let store = store().await;
        let first = acquire(&store, "a", 1_000).await.expect("first wins");
        assert_eq!(first.fence(), 1);

        assert!(
            DbLease::acquire_at(store.clone(), "b", Duration::from_secs(10), 1_500)
                .await
                .unwrap()
                .is_none(),
            "a live lease must not be taken over"
        );
        // The holder's own lease is untouched by the attempt.
        first.renew_at(1_500).await.unwrap();
        assert_eq!(first.fence(), 1);
    }

    /// Expiry takeover, and the fence that makes the old holder detectable:
    /// the successor's fence is strictly higher, and the old holder's renewal
    /// is refused instead of extending a lease it no longer owns.
    #[tokio::test]
    async fn an_expired_lease_is_taken_over_with_a_higher_fence() {
        let store = store().await;
        let first = acquire(&store, "a", 1_000).await.expect("first wins");
        let first_bytes = raw(&store, LEASE_KEY).await.unwrap();

        // Still live at the deadline minus one millisecond.
        assert!(
            acquire(&store, "b", 10_999).await.is_none(),
            "the lease is live until its deadline"
        );

        let second = acquire(&store, "b", 11_000)
            .await
            .expect("takeover at expiry");
        assert_eq!(second.fence(), 2, "a takeover must bump the fence");

        let err = first.renew_at(11_000).await.unwrap_err();
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");
        assert!(first.is_lost());
        // The successor's row is still the successor's, and its fence is
        // unchanged by the stale holder's failed renewal.
        let stored = raw(&store, LEASE_KEY).await.unwrap();
        assert_ne!(stored, first_bytes);
        assert_eq!(parse_record(&stored).unwrap().fence, 2);

        // Fences keep growing for every later acquisition.
        second.release().await.unwrap();
        let third = acquire(&store, "c", 12_000).await.expect("after release");
        assert_eq!(third.fence(), 3);
    }

    /// The heart of the invariant: an instance that lost the lease cannot
    /// write through its fenced store, and the refused write left nothing
    /// behind.
    #[tokio::test]
    async fn a_stale_fence_cannot_write() {
        let store = store().await;
        let stale = acquire(&store, "a", 1_000).await.unwrap();
        let stale_store = stale.fenced();
        // While it holds the lease, its writes land.
        stale_store.put("row", b"from-a".to_vec()).await.unwrap();
        assert_eq!(
            raw(&store, "row").await,
            Some(b"from-a".to_vec()),
            "the holder writes"
        );

        // It stalls past its deadline; a second instance takes over.
        let holder = acquire(&store, "b", 11_000).await.expect("takeover");
        assert_eq!(holder.fence(), 2);

        let err = stale_store
            .put("row", b"from-stale".to_vec())
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");
        assert!(
            stale.is_lost(),
            "a refused write proves the lease is gone, so the flag is set"
        );
        // Nothing of the stale instance landed — not the single write and not
        // a batch.
        assert_eq!(raw(&store, "row").await, Some(b"from-a".to_vec()));
        let err = stale_store
            .batch(
                &[
                    StoreBatchOp::Put {
                        key: "row2".to_string(),
                        value: b"from-stale".to_vec(),
                    },
                    StoreBatchOp::Delete {
                        key: "row".to_string(),
                    },
                ],
                &[],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");
        assert_eq!(raw(&store, "row").await, Some(b"from-a".to_vec()));
        assert!(raw(&store, "row2").await.is_none());

        // ... and every later write is refused without even asking the store.
        let err = stale_store.put("row3", b"x".to_vec()).await.unwrap_err();
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");
        assert!(raw(&store, "row3").await.is_none());

        // The live holder still writes normally.
        let holder_store = holder.fenced();
        holder_store.put("row", b"from-b".to_vec()).await.unwrap();
        assert_eq!(raw(&store, "row").await, Some(b"from-b".to_vec()));
    }

    /// A release by a stale holder must not delete the successor's lease:
    /// the guard is the releasing instance's own row bytes.
    #[tokio::test]
    async fn a_stale_release_does_not_delete_the_successor() {
        let store = store().await;
        let stale = acquire(&store, "a", 1_000).await.unwrap();
        // The holder takes the expired row over, so the row now carries the
        // successor's bytes and a higher fence.
        let holder = acquire(&store, "b", 11_000).await.expect("takeover");
        assert_eq!(holder.fence(), 2);

        // The stale instance tries to give up a lease it no longer has: the
        // release is guarded by ITS bytes, so it cannot touch the successor's.
        stale.release().await.unwrap();

        let stored = raw(&store, LEASE_KEY).await.unwrap();
        let row = parse_record(&stored).unwrap();
        assert_eq!(
            row.fence,
            holder.fence(),
            "the successor's lease survived the stale release"
        );
        assert_eq!(row.owner, "b");
        holder.renew_at(11_000).await.unwrap();
    }

    /// Renewal keeps the fence and moves the deadline: a long-lived instance
    /// is never taken over while it renews.
    #[tokio::test]
    async fn renewal_extends_the_deadline_without_bumping_the_fence() {
        let store = store().await;
        let lease = DbLease::acquire_at(store.clone(), "a", Duration::from_secs(10), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lease.expires_at(), 11_000);

        lease.renew_at(5_000).await.unwrap();
        assert_eq!(lease.fence(), 1);
        assert_eq!(lease.expires_at(), 15_000);

        // The old deadline has passed, but the lease is live: no takeover.
        assert!(acquire(&store, "b", 12_000).await.is_none());
    }

    /// A release frees the row for an immediate takeover, and the next
    /// acquisition — a restart of the same deployment — gets a higher fence, so
    /// a write from before the restart can never be mistaken for the new
    /// holder's.
    #[tokio::test]
    async fn release_frees_the_row_for_the_next_holder() {
        let store = store().await;
        let first = acquire(&store, "a", 1_000).await.unwrap();
        assert_eq!(first.fence(), 1);
        first.release().await.unwrap();
        let vacant = parse_record(&raw(&store, LEASE_KEY).await.unwrap()).unwrap();
        assert!(vacant.is_vacant(), "the row is left vacant: {vacant:?}");
        assert_eq!(
            vacant.fence, 1,
            "the fence is kept so the sequence never restarts"
        );
        assert!(first.is_lost(), "a released lease does not write again");

        let err = first.fenced().put("row", b"x".to_vec()).await.unwrap_err();
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");

        let second = acquire(&store, "a", 1_001).await.unwrap();
        assert_eq!(second.fence(), 2, "the fence survives a restart");
    }

    /// Every contender races the same row; exactly one wins, and the losers
    /// see `None` rather than an error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_acquisitions_elect_exactly_one_holder() {
        let store = store().await;
        let mut handles = Vec::new();
        for n in 0..16 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                DbLease::acquire_at(
                    store,
                    &format!("instance-{n}"),
                    Duration::from_secs(10),
                    1_000,
                )
                .await
                .unwrap()
            }));
        }
        let mut winners = Vec::new();
        for handle in handles {
            if let Some(lease) = handle.await.unwrap() {
                winners.push(lease);
            }
        }
        assert_eq!(winners.len(), 1, "exactly one instance may hold the lease");
        assert_eq!(winners[0].fence(), 1);

        // The winner keeps the lease, so a later round (after it is gone)
        // starts from a higher fence.
        winners[0].release().await.unwrap();
        let next = acquire(&store, "later", 2_000).await.unwrap();
        assert_eq!(next.fence(), 2);
    }

    /// Two instances racing a `create` under the same document id: the guard
    /// makes the second one's write fail rather than interleave, which is what
    /// the lease buys for the engine's read-modify-write document writes.
    #[tokio::test]
    async fn a_stale_guard_reports_no_write_and_applies_nothing() {
        let store = store().await;
        store.put("doc", b"v1".to_vec()).await.unwrap();

        let applied = store
            .batch(
                &[StoreBatchOp::Put {
                    key: "doc".to_string(),
                    value: b"v2".to_vec(),
                }],
                &[StoreGuard::holds("doc", b"v1".to_vec())],
            )
            .await
            .unwrap();
        assert!(applied);
        assert_eq!(raw(&store, "doc").await, Some(b"v2".to_vec()));

        // The same guard again: the row moved, so nothing applies.
        let conflict = store
            .batch(
                &[
                    StoreBatchOp::Put {
                        key: "doc".to_string(),
                        value: b"v3".to_vec(),
                    },
                    StoreBatchOp::Put {
                        key: "index".to_string(),
                        value: b"stale".to_vec(),
                    },
                ],
                &[StoreGuard::holds("doc", b"v1".to_vec())],
            )
            .await
            .unwrap();
        assert!(!conflict, "a stale guard applies nothing");
        assert_eq!(raw(&store, "doc").await, Some(b"v2".to_vec()));
        assert!(raw(&store, "index").await.is_none());

        // Absence is a guard of its own.
        let created = store
            .batch(
                &[StoreBatchOp::Put {
                    key: "fresh".to_string(),
                    value: b"1".to_vec(),
                }],
                &[StoreGuard::absent("fresh")],
            )
            .await
            .unwrap();
        assert!(created);
        let occupied = store
            .batch(
                &[StoreBatchOp::Put {
                    key: "fresh".to_string(),
                    value: b"2".to_vec(),
                }],
                &[StoreGuard::absent("fresh")],
            )
            .await
            .unwrap();
        assert!(!occupied);
        assert_eq!(raw(&store, "fresh").await, Some(b"1".to_vec()));
    }

    /// A store without an atomic guarded batch must not hand out a lease at
    /// all: acquiring on it is an error, not a best-effort lock.
    #[tokio::test]
    async fn a_backend_without_guarded_batches_cannot_lease() {
        struct NoGuards(Arc<dyn KvStore>);

        #[async_trait::async_trait]
        impl KvStore for NoGuards {
            async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
                self.0.one(key).await
            }
            async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
                self.0.put(key, value).await
            }
            async fn delete(&self, key: &str) -> Result<()> {
                self.0.delete(key).await
            }
            async fn scan_prefix(
                &self,
                key: &str,
                options: crate::store::ScanOptions,
            ) -> Result<Vec<(String, Vec<u8>)>> {
                self.0.scan_prefix(key, options).await
            }
        }

        let store: Arc<dyn KvStore> = Arc::new(NoGuards(store().await));
        // The store has no guard-able batch: its guardless batches work, and a
        // guarded one is refused instead of degrading.
        assert!(
            store
                .batch(
                    &[StoreBatchOp::Put {
                        key: "k".to_string(),
                        value: b"v".to_vec(),
                    }],
                    &[]
                )
                .await
                .unwrap()
        );
        let err = DbLease::acquire(store, "a", Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::Store(_)), "got: {err}");
        assert!(err.to_string().contains("guarded batch"), "got: {err}");
    }

    /// Renewal failing because the store is unreachable does not lose the
    /// lease: the guarded renewal, not the clock, decides who holds the row,
    /// so only a real takeover does.
    #[tokio::test]
    async fn a_store_failure_during_renewal_does_not_lose_the_lease() {
        /// Fails every guarded batch once armed; reads keep working.
        struct FlakyKv {
            inner: Arc<dyn KvStore>,
            fail: AtomicBool,
        }

        #[async_trait::async_trait]
        impl KvStore for FlakyKv {
            async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
                self.inner.one(key).await
            }
            async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
                self.inner.put(key, value).await
            }
            async fn delete(&self, key: &str) -> Result<()> {
                self.inner.delete(key).await
            }
            async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(ActError::Store("store unavailable".to_string()));
                }
                self.inner.batch(ops, guards).await
            }
            async fn scan_prefix(
                &self,
                key: &str,
                options: crate::store::ScanOptions,
            ) -> Result<Vec<(String, Vec<u8>)>> {
                self.inner.scan_prefix(key, options).await
            }
        }

        let flaky = Arc::new(FlakyKv {
            inner: store().await,
            fail: AtomicBool::new(false),
        });
        let fenced: Arc<dyn KvStore> = flaky.clone();
        let lease = DbLease::acquire_at(fenced, "a", Duration::from_secs(10), 1_000)
            .await
            .unwrap()
            .unwrap();

        // Failing before the deadline: not lost, and the caller sees the
        // store error so it can retry.
        flaky.fail.store(true, Ordering::SeqCst);
        let err = lease.renew_at(2_000).await.unwrap_err();
        assert!(matches!(err, ActError::Store(_)), "got: {err}");
        assert!(!lease.is_lost());
        assert!(!lease.is_expired_at(2_000));

        // Failing past the deadline: the lease is past its deadline and
        // cannot be confirmed, which is the state the keeper treats as lost.
        let err = lease.renew_at(11_000).await.unwrap_err();
        assert!(matches!(err, ActError::Store(_)), "got: {err}");
        assert!(lease.is_expired_at(11_000));
        assert!(
            lease.is_expired(),
            "a lease past its deadline is not usable"
        );

        // The deadline alone does not take the lease away: once the store
        // answers again, the guarded renewal proves the row is still this
        // instance's (a takeover would have replaced it and its fence), so the
        // lease stays usable instead of being abandoned mid-flight.
        flaky.fail.store(false, Ordering::SeqCst);
        lease.renew_at(12_000).await.unwrap();
        assert!(!lease.is_lost());
        assert_eq!(lease.fence(), 1, "reclaiming keeps the fence");

        // A holder whose lease WAS taken over cannot renew, however healthy
        // the store is. (A fresh database: the lease above still holds this
        // one's row until its renewed deadline.)
        let clean = store().await;
        let lease = DbLease::acquire_at(clean.clone(), "a", Duration::from_secs(10), 1_000)
            .await
            .unwrap()
            .unwrap();
        let _thief = DbLease::acquire_at(clean.clone(), "b", Duration::from_secs(10), 11_000)
            .await
            .unwrap()
            .unwrap();
        let err = lease.renew_at(12_000).await.unwrap_err();
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");
        assert!(lease.is_lost());
    }

    /// The keeper renews on its interval, releases the row on a graceful stop,
    /// and leaves the next instance the higher fence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_keeper_renews_and_releases_the_lease() {
        let store = store().await;
        let lease = Arc::new(acquire(&store, "keeper", 1_000).await.unwrap());
        let shutdown = CancellationToken::new();
        let keeper = LeaseKeeper::start(lease.clone(), Duration::from_millis(5), shutdown.clone());

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!shutdown.is_cancelled());
        assert!(!lease.is_lost());
        assert_eq!(lease.fence(), 1, "renewal never changes the fence");

        keeper.stop().await;
        assert!(shutdown.is_cancelled());
        let vacant = parse_record(&raw(&store, LEASE_KEY).await.expect("the row stays")).unwrap();
        assert!(
            vacant.is_vacant(),
            "a graceful stop hands the lease back: {vacant:?}"
        );
        let next = acquire(&store, "next", 2_000).await.unwrap();
        assert_eq!(next.fence(), 2, "the fence continues across the hand-over");
    }

    /// A renewal that finds the lease taken over stops the engine: the lease
    /// is marked lost and the shutdown token fires.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_keeper_stops_the_engine_when_renewal_is_refused() {
        let store = store().await;
        let lease = Arc::new(acquire(&store, "keeper", 1_000).await.unwrap());
        let shutdown = CancellationToken::new();
        let keeper = LeaseKeeper::start(lease.clone(), Duration::from_millis(5), shutdown.clone());

        // Another instance takes the row over behind the keeper's back, as a
        // takeover after an expiry would.
        store
            .put(
                LEASE_KEY,
                serde_json::to_vec(&LeaseRecord {
                    owner: "thief".to_string(),
                    fence: lease.fence() + 1,
                    expires_at: 60_000,
                })
                .unwrap(),
            )
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if shutdown.is_cancelled() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the keeper must stop the engine when its lease is taken over");
        assert!(lease.is_lost());
        assert!(matches!(
            lease.fenced().put("row", b"x".to_vec()).await.unwrap_err(),
            ActError::LeaseLost
        ));
        keeper.stop().await;
    }
}
