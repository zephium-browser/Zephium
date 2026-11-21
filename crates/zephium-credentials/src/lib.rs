//! Safe, bounded OS vault operations for native secret owners: Windows
//! Credential Manager, and generic passwords in the macOS login Keychain.
#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Content-free native vault failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultError {
    /// The exact credential does not exist.
    Missing,
    /// The OS refused access or returned an invalid record.
    Inaccessible,
    /// The target or blob exceeds the native contract.
    Invalid,
    /// A bounded allocation failed.
    Capacity,
    /// An earlier vault call is still waiting on an OS prompt the person has
    /// not answered; this call never reached the vault.
    AwaitingApproval,
}

/// How long a vault call waits for the call before it. A Keychain call takes
/// milliseconds, unless the OS is showing the person a prompt for it, which
/// stays up for as long as they are away.
pub const TURN_PATIENCE: Duration = Duration::from_secs(4);

static BUSY: Mutex<bool> = Mutex::new(false);
static FREED: Condvar = Condvar::new();

/// One process-wide vault turn; dropping it lets the next call in.
#[must_use]
pub struct Turn(());

impl Drop for Turn {
    fn drop(&mut self) {
        *BUSY.lock().unwrap_or_else(PoisonError::into_inner) = false;
        FREED.notify_one();
    }
}

impl std::fmt::Debug for Turn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Turn")
    }
}

/// Process-wide serialization shared by provider and connection vault calls,
/// waiting at most [`TURN_PATIENCE`] for the call ahead.
pub fn turn() -> Result<Turn, VaultError> {
    turn_within(TURN_PATIENCE)
}

/// [`turn`] with an explicit bound on the wait.
pub fn turn_within(patience: Duration) -> Result<Turn, VaultError> {
    let deadline = Instant::now() + patience;
    let mut busy = BUSY.lock().unwrap_or_else(PoisonError::into_inner);
    while *busy {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(VaultError::AwaitingApproval);
        }
        busy = FREED
            .wait_timeout(busy, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    *busy = true;
    Ok(Turn(()))
}

#[cfg(any(windows, target_os = "macos", test))]
fn isolated_identity(identifier: &str) -> Option<&str> {
    match identifier {
        "app.zephium" | "app.zephium.dev" => None,
        other => Some(other),
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{delete, list, present, read, set_windows_identity, write};

#[cfg(any(windows, test))]
mod windows_target;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{delete, present, read, write};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_held_behind_a_prompt_fails_later_calls_fast_and_frees_on_drop() {
        let held = turn().expect("first turn");
        let waiter = std::thread::spawn(|| {
            let started = Instant::now();
            let result = turn_within(Duration::from_millis(50)).map(drop);
            (result, started.elapsed())
        });
        let (result, waited) = waiter.join().expect("waiter");
        assert_eq!(result, Err(VaultError::AwaitingApproval));
        assert!(waited < Duration::from_secs(2));
        let queued = std::thread::spawn(|| turn_within(Duration::from_secs(5)).map(drop));
        std::thread::sleep(Duration::from_millis(20));
        drop(held);
        assert_eq!(queued.join().expect("queued"), Ok(()));
        assert!(turn_within(Duration::ZERO).is_ok());
    }
}
