# Product principles

Zephium is a native Rust browser with a Tauri + Svelte interface. Windows and
macOS have equal support priority. Linux is not supported.

## Contribution scope

Security, resource use and usability shape the engineering choices. Discuss
changes to defaults, new capabilities and cross-subsystem designs before
implementation; see [CONTRIBUTING](../CONTRIBUTING.md).

- Build on the operating system's native web engine: WebView2 on Windows and
  WebKit on macOS. Keep privileged browser controls separate from page content.
- Keep durable state, permissions and operation settlement in Rust. Interface
  components present that state and submit intent.
- Keep browsing profiles and private-session data scoped to their owners.
  Changes to data flow or external services need explicit review.
- Make work observable and controllable. An accepted request is not proof of
  completion; failures, cancellation and recovery must remain distinguishable.
- Prefer bounded resource use and shared components over duplicate state or
  additional infrastructure. Measure changes to hot paths on the affected hosts.
- Preserve keyboard access, focus, reduced motion and both appearance modes.
  Native platform differences are intentional where the engines require them.

## Engineering references

- [Architecture](architecture.md): process model, crate ownership and native boundaries.
- [Frontend](frontend.md): interface structure and component contracts.
- [Work](work.md): execution composition, persistence and the sidebar assistant.
- [Browser execution](agentic-browsing.md): observation, action and authority.
- [Security model](security-model.md): security design and review material.

Code, tests and scoped native evidence establish current behavior. These
principles do not certify platform qualification or override implemented checks.
