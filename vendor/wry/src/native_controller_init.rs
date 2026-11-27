// Copyright 2026 Zephium contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Controller configuration has no authority to change extension enablement.

type Hook<'a, E, C, Error> = &'a dyn Fn(&E, &C) -> Result<(), Error>;

pub(crate) fn run<E, C, Error>(
  environment: &E,
  controller: &C,
  extension_gate: Option<Hook<'_, E, C, Error>>,
  controller_hook: Option<Hook<'_, E, C, Error>>,
) -> Result<(), Error> {
  if let Some(gate) = extension_gate {
    gate(environment, controller)?;
  }
  if let Some(hook) = controller_hook {
    hook(environment, controller)?;
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::cell::{Cell, RefCell};

  #[test]
  fn extension_disabled_default_private_and_agent_controllers_still_configure_reputation() {
    for view in ["default", "private", "agent"] {
      let extensions_enabled = Cell::new(false);
      let reputation_applied = Cell::new(false);
      let configure = |enabled: &Cell<bool>, actual_view: &&str| {
        assert!(
          !enabled.get(),
          "{actual_view} unexpectedly enabled extensions"
        );
        reputation_applied.set(true);
        Ok::<(), ()>(())
      };
      run(&extensions_enabled, &view, None, Some(&configure)).unwrap();
      assert!(
        reputation_applied.get(),
        "{view} omitted reputation configuration"
      );
      assert!(!extensions_enabled.get());
    }
  }

  #[test]
  fn extension_admission_precedes_configuration_and_refusal_stops_it() {
    let events = RefCell::new(Vec::new());
    let authenticate = |_: &(), _: &()| {
      events.borrow_mut().push("admit");
      Ok(())
    };
    let configure = |_: &(), _: &()| {
      events.borrow_mut().push("configure");
      Ok(())
    };
    run::<_, _, &str>(&(), &(), Some(&authenticate), Some(&configure)).unwrap();
    assert_eq!(*events.borrow(), ["admit", "configure"]);
    events.borrow_mut().clear();
    let refuse = |_: &(), _: &()| Err("unadmitted extension environment");
    assert!(run(&(), &(), Some(&refuse), Some(&configure)).is_err());
    assert!(events.borrow().is_empty());
  }

  #[test]
  fn configuration_failure_is_returned_for_owned_construction_cleanup() {
    let fail = |_: &(), _: &()| Err("controller configuration failed");
    assert_eq!(
      run(&(), &(), None, Some(&fail)),
      Err("controller configuration failed")
    );
  }
}
