//! What a tab's media is doing, as its page reports it, and which tab the
//! sidebar's player follows.
//!
//! The player follows the tab that most recently started playing, as long as
//! it is not on screen, has not finished, and the person has not closed the
//! player for what it is playing now. Nothing here is persisted.

use std::collections::HashMap;
use std::time::Instant;

use crate::ids::ItemId;

/// Longest title or artist kept, in characters.
pub const MAX_MEDIA_TEXT: usize = 200;
const MAX_REPORT_BYTES: usize = 4096;
/// Longest stretch of media treated as finite: a day.
const MAX_DURATION: f64 = 86_400.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MediaSnapshot {
    /// From the page's Media Session; empty when it names none.
    pub title: String,
    pub artist: String,
    /// Seconds; `None` while unknown or for a live stream.
    pub duration: Option<f64>,
    pub position: f64,
    pub rate: f64,
    pub playing: bool,
    /// Playing, but waiting for data.
    pub waiting: bool,
    pub ended: bool,
    pub can_pip: bool,
    pub in_pip: bool,
    /// Names what is playing, so a closed player stays away until something
    /// else plays.
    pub item: String,
}

impl MediaSnapshot {
    /// Where playback is `elapsed` seconds after this report, assuming it went
    /// on as reported.
    pub fn position_after(&self, elapsed: f64) -> f64 {
        let moving = self.playing && !self.waiting && elapsed.is_finite() && elapsed > 0.0;
        let position = self.position + if moving { elapsed * self.rate } else { 0.0 };
        self.duration
            .map_or(position, |duration| position.min(duration))
    }
}

/// One answer of the page's media watch: the state token it resumes from, and
/// the media, if the page has any it can control.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaReport {
    pub token: String,
    pub media: Option<MediaSnapshot>,
}

fn token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 16 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn text(value: &str) -> String {
    let mut clean = String::new();
    for word in value.split(|c: char| c.is_whitespace() || c.is_control()) {
        if word.is_empty() {
            continue;
        }
        if !clean.is_empty() {
            clean.push(' ');
        }
        clean.push_str(word);
    }
    clean.chars().take(MAX_MEDIA_TEXT).collect()
}

/// Reads the page's report: `{"k": token, "m": null | {"t", "a", "d", "p",
/// "r", "s", "w", "e", "q", "i", "n"}}`. Page script wrote it, so anything
/// malformed is refused whole and every value is bounded.
pub fn parse_report(json: &str) -> Option<MediaReport> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Media {
        t: String,
        a: String,
        d: Option<f64>,
        p: f64,
        r: f64,
        s: u8,
        w: u8,
        e: u8,
        q: u8,
        i: u8,
        n: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        k: String,
        m: Option<Media>,
    }
    if json.len() > MAX_REPORT_BYTES {
        return None;
    }
    let wire: Wire = serde_json::from_str(json).ok()?;
    if !token(&wire.k) {
        return None;
    }
    let media = match wire.m {
        None => None,
        Some(m) => {
            if !token(&m.n)
                || [m.s, m.w, m.e, m.q, m.i].iter().any(|flag| *flag > 1)
                || !m.p.is_finite()
                || !m.r.is_finite()
            {
                return None;
            }
            let duration =
                m.d.filter(|duration| duration.is_finite() && *duration > 0.0)
                    .filter(|duration| *duration <= MAX_DURATION);
            let position = m.p.clamp(0.0, duration.unwrap_or(MAX_DURATION));
            Some(MediaSnapshot {
                title: text(&m.t),
                artist: text(&m.a),
                duration,
                position,
                rate: m.r.clamp(0.0, 16.0),
                playing: m.s == 1,
                waiting: m.w == 1,
                ended: m.e == 1,
                can_pip: m.q == 1,
                in_pip: m.i == 1,
                item: m.n,
            })
        }
    };
    Some(MediaReport {
        token: wire.k,
        media,
    })
}

#[derive(Debug)]
struct TabMedia {
    snapshot: MediaSnapshot,
    /// When the page reported it.
    at: Instant,
    /// When it last started playing; zero until it has.
    started: u64,
    /// The item the person closed the player on.
    dismissed: Option<String>,
}

/// Every tab's reported media and the order they started playing in.
#[derive(Debug, Default)]
pub struct MediaPlayers {
    tabs: HashMap<ItemId, TabMedia>,
    clock: u64,
}

impl MediaPlayers {
    pub fn get(&self, id: ItemId) -> Option<&MediaSnapshot> {
        self.tabs.get(&id).map(|tab| &tab.snapshot)
    }

    /// Applies a page's report; `None` means the page has no media it can
    /// control. Returns whether anything changed.
    pub fn observe(&mut self, id: ItemId, media: Option<MediaSnapshot>, now: Instant) -> bool {
        let Some(snapshot) = media else {
            return self.forget(id);
        };
        let clock = &mut self.clock;
        let fresh = !self.tabs.contains_key(&id);
        let entry = self.tabs.entry(id).or_insert_with(|| TabMedia {
            snapshot: snapshot.clone(),
            at: now,
            started: 0,
            dismissed: None,
        });
        let resumed = snapshot.playing
            && (entry.started == 0
                || !entry.snapshot.playing
                || entry.snapshot.item != snapshot.item);
        if resumed {
            *clock += 1;
            entry.started = *clock;
        }
        if snapshot.playing
            && entry
                .dismissed
                .as_ref()
                .is_some_and(|item| *item != snapshot.item)
        {
            entry.dismissed = None;
        }
        let changed = fresh || resumed || entry.snapshot != snapshot;
        entry.snapshot = snapshot;
        entry.at = now;
        changed
    }

    /// The tab closed, or its page and its media went away.
    pub fn forget(&mut self, id: ItemId) -> bool {
        self.tabs.remove(&id).is_some()
    }

    /// The person closed the player on this tab's current item.
    pub fn dismiss(&mut self, id: ItemId) -> bool {
        let Some(tab) = self.tabs.get_mut(&id) else {
            return false;
        };
        let item = Some(tab.snapshot.item.clone());
        let changed = tab.dismissed != item;
        tab.dismissed = item;
        changed
    }

    /// The tab the player shows: the most recent to start playing among those
    /// `eligible` (the person's own tabs, of this window's profile) that are
    /// not `on_screen`, not finished, and not closed by the person, with when
    /// its page last reported.
    pub fn shown(
        &self,
        eligible: impl Fn(ItemId) -> bool,
        on_screen: impl Fn(ItemId) -> bool,
    ) -> Option<(ItemId, &MediaSnapshot, Instant)> {
        self.tabs
            .iter()
            .filter(|(id, tab)| {
                tab.started > 0
                    && !tab.snapshot.ended
                    && tab.dismissed.is_none()
                    && eligible(**id)
                    && !on_screen(**id)
            })
            .max_by_key(|(_, tab)| tab.started)
            .map(|(id, tab)| (*id, &tab.snapshot, tab.at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(item: &str, playing: bool) -> MediaSnapshot {
        MediaSnapshot {
            title: "Song".into(),
            artist: "Band".into(),
            duration: Some(180.0),
            position: 12.0,
            rate: 1.0,
            playing,
            waiting: false,
            ended: false,
            can_pip: false,
            in_pip: false,
            item: item.into(),
        }
    }

    fn shown(players: &MediaPlayers, front: &[ItemId]) -> Option<ItemId> {
        players
            .shown(|_| true, |id| front.contains(&id))
            .map(|(id, ..)| id)
    }

    #[test]
    fn a_report_is_read_whole_and_bounded() {
        let report = parse_report(
            r#"{"k":"1f","m":{"t":"  A\n\tsong ","a":"Band","d":200.5,"p":250,"r":1,"s":1,"w":0,"e":0,"q":1,"i":0,"n":"ab12"}}"#,
        )
        .unwrap();
        assert_eq!(report.token, "1f");
        let media = report.media.unwrap();
        assert_eq!(media.title, "A song");
        assert_eq!(media.duration, Some(200.5));
        assert_eq!(media.position, 200.5);
        assert!(media.playing && media.can_pip && !media.in_pip);
        assert_eq!(
            parse_report(r#"{"k":"00","m":null}"#),
            Some(MediaReport {
                token: "00".into(),
                media: None
            })
        );
        let live = parse_report(
            r#"{"k":"1","m":{"t":"","a":"","d":null,"p":5,"r":1,"s":1,"w":1,"e":0,"q":0,"i":0,"n":"2"}}"#,
        )
        .unwrap()
        .media
        .unwrap();
        assert_eq!(live.duration, None);
        assert!(live.waiting);
        let long = "x".repeat(1000);
        let titled = parse_report(&format!(
            r#"{{"k":"1","m":{{"t":"{long}","a":"","d":1e300,"p":-4,"r":99,"s":0,"w":0,"e":0,"q":0,"i":0,"n":"2"}}}}"#
        ))
        .unwrap()
        .media
        .unwrap();
        assert_eq!(titled.title.chars().count(), MAX_MEDIA_TEXT);
        assert_eq!(titled.duration, None);
        assert_eq!(titled.position, 0.0);
        assert_eq!(titled.rate, 16.0);
    }

    #[test]
    fn a_malformed_report_is_refused() {
        for bad in [
            "",
            "null",
            r#"{"k":"","m":null}"#,
            r#"{"k":"zz","m":null}"#,
            r#"{"k":"12345678901234567","m":null}"#,
            r#"{"k":"1","m":null,"x":1}"#,
            r#"{"k":"1","m":{"t":"","a":"","d":1,"p":0,"r":1,"s":2,"w":0,"e":0,"q":0,"i":0,"n":"2"}}"#,
            r#"{"k":"1","m":{"t":"","a":"","d":1,"p":0,"r":1,"s":1,"w":0,"e":0,"q":0,"i":0,"n":"<b>"}}"#,
            r#"{"k":"1","m":{"t":"","a":"","d":1,"p":0,"r":1,"s":1,"w":0,"e":0,"q":0,"i":0}}"#,
        ] {
            assert_eq!(parse_report(bad), None, "{bad}");
        }
        assert_eq!(
            parse_report(&format!(
                r#"{{"k":"1","m":null,"p":"{}"}}"#,
                "x".repeat(5000)
            )),
            None
        );
    }

    #[test]
    fn the_player_follows_the_latest_tab_to_start_playing_off_screen() {
        let (a, b) = (ItemId::generate(), ItemId::generate());
        let mut players = MediaPlayers::default();
        players.observe(a, Some(media("1", true)), Instant::now());
        assert_eq!(shown(&players, &[]), Some(a));
        assert_eq!(shown(&players, &[a]), None);
        players.observe(b, Some(media("2", true)), Instant::now());
        assert_eq!(shown(&players, &[]), Some(b));
        assert_eq!(shown(&players, &[b]), Some(a));
        // Pausing keeps the player, so it can resume; playing again is news.
        players.observe(a, Some(media("1", false)), Instant::now());
        assert_eq!(shown(&players, &[]), Some(b));
        players.observe(a, Some(media("1", true)), Instant::now());
        assert_eq!(shown(&players, &[]), Some(a));
    }

    #[test]
    fn a_tab_that_has_never_played_or_has_finished_is_not_shown() {
        let a = ItemId::generate();
        let mut players = MediaPlayers::default();
        assert!(players.observe(a, Some(media("1", false)), Instant::now()));
        assert_eq!(shown(&players, &[]), None);
        players.observe(a, Some(media("1", true)), Instant::now());
        assert_eq!(shown(&players, &[]), Some(a));
        let mut ended = media("1", false);
        ended.ended = true;
        players.observe(a, Some(ended), Instant::now());
        assert_eq!(shown(&players, &[]), None);
        assert!(players.observe(a, None, Instant::now()));
        assert!(players.get(a).is_none());
        assert!(!players.observe(a, None, Instant::now()));
    }

    #[test]
    fn closing_the_player_holds_until_something_new_plays() {
        let a = ItemId::generate();
        let mut players = MediaPlayers::default();
        players.observe(a, Some(media("1", true)), Instant::now());
        assert!(players.dismiss(a));
        assert!(!players.dismiss(a));
        assert_eq!(shown(&players, &[]), None);
        players.observe(a, Some(media("1", false)), Instant::now());
        players.observe(a, Some(media("1", true)), Instant::now());
        assert_eq!(shown(&players, &[]), None);
        // A new item that is only loaded, not yet playing, stays away too.
        players.observe(a, Some(media("2", false)), Instant::now());
        assert_eq!(shown(&players, &[]), None);
        players.observe(a, Some(media("2", true)), Instant::now());
        assert_eq!(shown(&players, &[]), Some(a));
    }

    #[test]
    fn only_eligible_tabs_are_shown_and_closed_tabs_are_forgotten() {
        let (a, b) = (ItemId::generate(), ItemId::generate());
        let mut players = MediaPlayers::default();
        players.observe(a, Some(media("1", true)), Instant::now());
        players.observe(b, Some(media("2", true)), Instant::now());
        assert_eq!(
            players.shown(|id| id == a, |_| false).map(|(id, ..)| id),
            Some(a)
        );
        assert!(players.forget(b));
        assert_eq!(shown(&players, &[]), Some(a));
        assert!(!players.forget(b));
    }

    #[test]
    fn playback_runs_on_from_the_report_only_while_it_plays() {
        let mut playing = media("1", true);
        playing.rate = 2.0;
        assert_eq!(playing.position_after(3.0), 18.0);
        assert_eq!(playing.position_after(1000.0), 180.0);
        assert_eq!(playing.position_after(f64::NAN), 12.0);
        let mut waiting = playing.clone();
        waiting.waiting = true;
        assert_eq!(waiting.position_after(3.0), 12.0);
        assert_eq!(media("1", false).position_after(3.0), 12.0);
    }

    #[test]
    fn an_unchanged_report_changes_nothing() {
        let a = ItemId::generate();
        let mut players = MediaPlayers::default();
        assert!(players.observe(a, Some(media("1", true)), Instant::now()));
        assert!(!players.observe(a, Some(media("1", true)), Instant::now()));
        let mut moved = media("1", true);
        moved.position = 40.0;
        assert!(players.observe(a, Some(moved), Instant::now()));
    }
}
