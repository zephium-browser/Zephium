// Drafts and the other small things the chrome remembers live in this origin's
// storage, which every test file of the run shares; none may reach the next.
try {
  localStorage.clear();
} catch {
  // Storage blocked: nothing was kept.
}
