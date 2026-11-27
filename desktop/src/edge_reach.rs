//! When a pointer is at, or plainly heading for, a window's left edge, where
//! a hidden sidebar waits. Each platform reads the pointer its own way and
//! asks the same question of it.

/// Within this of the window's left edge the pointer is at it: the gutter
/// before the page and a hair of the page's own edge.
const REACH: f64 = 10.0;
/// Further in, a pointer moving fast enough to cross the edge within the
/// next couple of moves is taken as already there, so the column is on its
/// way before the pointer lands.
const APPROACH: f64 = 56.0;
const LEAD: f64 = 2.0;
/// How far past the reach the pointer is still at the edge once there.
const LEAVE: f64 = 8.0;

/// Whether a pointer at `x` that last moved by `step` is at the edge. Once
/// there, it stays until it is a little way into the page, so a pointer
/// resting on the boundary does not flicker the column; any further and a
/// return to the edge would not be seen as one.
pub fn at_edge(x: f64, step: f64, near: bool) -> bool {
    if near && x <= REACH + LEAVE {
        return true;
    }
    x <= REACH || (x < APPROACH && step < 0.0 && x + step * LEAD <= 0.0)
}

#[cfg(test)]
mod tests {
    use super::at_edge;

    #[test]
    fn the_edge_is_reached_or_about_to_be() {
        assert!(at_edge(0.0, 0.0, false));
        assert!(at_edge(10.0, 3.0, false));
        assert!(!at_edge(30.0, 0.0, false));
        // Fast toward the edge: there within two moves.
        assert!(at_edge(40.0, -24.0, false));
        // Slowly toward it, or away from it, is not yet.
        assert!(!at_edge(40.0, -6.0, false));
        assert!(!at_edge(40.0, 24.0, false));
        // Once there, resting just inside keeps it; further in lets it go,
        // so coming back to the edge is seen again.
        assert!(at_edge(16.0, 0.0, true));
        assert!(!at_edge(30.0, 0.0, true));
        assert!(at_edge(4.0, 0.0, false));
    }
}
