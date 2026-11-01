//! Lock acquisition with an explicit poison rule.
//!
//! A `std::sync` lock is poisoned when a thread panics while holding it. Each call site states what
//! it does then: `*_or_panic` stops (the guarded state may be half updated, so continuing would act
//! on it), `*_or_recover` takes the state as the panicking thread left it (for state that is valid
//! after any partial update, or that the panic-isolation points expect to survive).

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

const POISONED: &str = "lock poisoned by an earlier panic";

/// Poison-explicit locking of a [`Mutex`].
pub trait MutexExt<T> {
    /// Locks, panicking if an earlier panic poisoned the lock.
    fn lock_or_panic(&self) -> MutexGuard<'_, T>;
    /// Locks, taking the state an earlier panic left if it poisoned the lock.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

#[expect(clippy::disallowed_methods, reason = "the one place that locks a std mutex")]
impl<T> MutexExt<T> for Mutex<T> {
    #[track_caller]
    fn lock_or_panic(&self) -> MutexGuard<'_, T> {
        self.lock().expect(POISONED)
    }

    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Poison-explicit locking of a [`RwLock`].
pub trait RwLockExt<T> {
    /// Locks for reading, panicking if an earlier panic poisoned the lock.
    fn read_or_panic(&self) -> RwLockReadGuard<'_, T>;
    /// Locks for writing, panicking if an earlier panic poisoned the lock.
    fn write_or_panic(&self) -> RwLockWriteGuard<'_, T>;
    /// Locks for reading, taking the state an earlier panic left.
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T>;
    /// Locks for writing, taking the state an earlier panic left.
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T>;
}

#[expect(clippy::disallowed_methods, reason = "the one place that locks a std rwlock")]
impl<T> RwLockExt<T> for RwLock<T> {
    #[track_caller]
    fn read_or_panic(&self) -> RwLockReadGuard<'_, T> {
        self.read().expect(POISONED)
    }

    #[track_caller]
    fn write_or_panic(&self) -> RwLockWriteGuard<'_, T> {
        self.write().expect(POISONED)
    }

    fn read_or_recover(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
