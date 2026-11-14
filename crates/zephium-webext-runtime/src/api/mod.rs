//! Namespace implementations, one module per namespace, each exposing
//! `register(&mut ApiTable<S>)` and listed in [`register`].

use crate::dispatch::{ApiTable, RegistrationError};

/// Registers every implemented namespace.
pub fn register<S>(_table: &mut ApiTable<S>) -> Result<(), RegistrationError> {
    Ok(())
}
