//! DOM objects remain inside the authenticated JavaScript world.

/// No serializable native DOM calls exist; this method is installed by bootstrap.
pub(crate) const METHODS: &[&str] = &[];

/// WebKitGTK exposes open roots. Closed roots are unavailable in isolated JSC worlds.
/// Capture the native getter before running untrusted content-script code.
pub const OPEN_OR_CLOSED_SHADOW_ROOT_BOOTSTRAP: &str = r#"(() => {
  const get = Object.getOwnPropertyDescriptor(Element.prototype, 'shadowRoot').get;
  return element => Reflect.apply(get, element, []);
})()"#;

/// Installs no native methods: live DOM identity cannot cross JSON transport.
pub fn register<S>(
    _: &mut crate::dispatch::ApiTable<S>,
) -> Result<(), crate::dispatch::RegistrationError> {
    debug_assert!(METHODS.is_empty());
    Ok(())
}
