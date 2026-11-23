use std::{
    collections::VecDeque,
    sync::{Mutex, PoisonError},
};

use zephium_map::PlaceKey;

pub const MAX_MEMO_PLACES: usize = 64;
pub const MAX_MEMO_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MEMO_PROFILES: usize = 16;
pub const MAX_PROFILE_KEY_BYTES: usize = 128;

pub trait Memo: Send + Sync {
    fn lookup(&self, profile: &str, site: &PlaceKey) -> Option<Vec<u8>>;
    fn remember(&self, profile: &str, site: &PlaceKey, memo: Vec<u8>);
}

#[derive(Default)]
struct Shelf {
    places: VecDeque<(PlaceKey, Vec<u8>)>,
    bytes: usize,
}

impl Shelf {
    fn remove(&mut self, site: &PlaceKey) {
        if let Some(at) = self.places.iter().position(|(key, _)| key == site) {
            if let Some((_, memo)) = self.places.remove(at) {
                self.bytes -= memo.len();
            }
        }
    }

    fn insert(&mut self, site: &PlaceKey, memo: Vec<u8>) {
        self.remove(site);
        self.bytes += memo.len();
        self.places.push_back((site.clone(), memo));
        while self.places.len() > MAX_MEMO_PLACES || self.bytes > MAX_MEMO_BYTES {
            let Some((_, oldest)) = self.places.pop_front() else {
                break;
            };
            self.bytes -= oldest.len();
        }
    }
}

#[derive(Default)]
pub struct MemoryMemo {
    profiles: Mutex<VecDeque<(String, Shelf)>>,
}

impl MemoryMemo {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Memo for MemoryMemo {
    fn lookup(&self, profile: &str, site: &PlaceKey) -> Option<Vec<u8>> {
        if profile.len() > MAX_PROFILE_KEY_BYTES {
            return None;
        }
        let profiles = self.profiles.lock().unwrap_or_else(PoisonError::into_inner);
        let (_, shelf) = profiles.iter().find(|(key, _)| key == profile)?;
        shelf
            .places
            .iter()
            .find(|(key, _)| key == site)
            .map(|(_, memo)| memo.clone())
    }

    fn remember(&self, profile: &str, site: &PlaceKey, memo: Vec<u8>) {
        if profile.len() > MAX_PROFILE_KEY_BYTES {
            return;
        }
        let mut profiles = self.profiles.lock().unwrap_or_else(PoisonError::into_inner);
        let mut shelf = match profiles.iter().position(|(key, _)| key == profile) {
            Some(at) => profiles
                .remove(at)
                .map(|(_, shelf)| shelf)
                .unwrap_or_default(),
            None => Shelf::default(),
        };
        if memo.len() > MAX_MEMO_BYTES {
            shelf.remove(site);
        } else {
            shelf.insert(site, memo);
        }
        profiles.push_back((profile.to_owned(), shelf));
        while profiles.len() > MAX_MEMO_PROFILES {
            profiles.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn site(n: usize) -> PlaceKey {
        PlaceKey::from_url(&Url::parse(&format!("https://site{n}.com/")).unwrap()).unwrap()
    }

    #[test]
    fn places_are_capped_and_the_oldest_goes_first() {
        let memo = MemoryMemo::new();
        for n in 0..=MAX_MEMO_PLACES {
            memo.remember("a", &site(n), vec![n as u8]);
        }
        assert_eq!(memo.lookup("a", &site(0)), None);
        assert_eq!(memo.lookup("a", &site(1)), Some(vec![1]));
        assert_eq!(
            memo.lookup("a", &site(MAX_MEMO_PLACES)),
            Some(vec![MAX_MEMO_PLACES as u8])
        );
        memo.remember("a", &site(1), vec![9]);
        memo.remember("a", &site(MAX_MEMO_PLACES + 1), vec![0]);
        assert_eq!(memo.lookup("a", &site(1)), Some(vec![9]));
        assert_eq!(memo.lookup("a", &site(2)), None);
    }

    #[test]
    fn bytes_are_capped_at_the_limit() {
        let memo = MemoryMemo::new();
        let quarter = MAX_MEMO_BYTES / 4;
        for n in 0..4 {
            memo.remember("a", &site(n), vec![0; quarter]);
        }
        assert!((0..4).all(|n| memo.lookup("a", &site(n)).is_some()));
        memo.remember("a", &site(4), vec![0; 1]);
        assert_eq!(memo.lookup("a", &site(0)), None);
        assert!(memo.lookup("a", &site(4)).is_some());
        memo.remember("a", &site(5), vec![0; MAX_MEMO_BYTES]);
        assert_eq!(
            memo.lookup("a", &site(5)).map(|m| m.len()),
            Some(MAX_MEMO_BYTES)
        );
        assert!((1..5).all(|n| memo.lookup("a", &site(n)).is_none()));
        memo.remember("a", &site(5), vec![0; MAX_MEMO_BYTES + 1]);
        assert_eq!(memo.lookup("a", &site(5)), None);
    }

    #[test]
    fn profiles_are_isolated_and_bounded() {
        let memo = MemoryMemo::new();
        memo.remember("a", &site(0), vec![1]);
        assert_eq!(memo.lookup("b", &site(0)), None);
        let long = "p".repeat(MAX_PROFILE_KEY_BYTES + 1);
        memo.remember(&long, &site(0), vec![1]);
        assert_eq!(memo.lookup(&long, &site(0)), None);
        let edge = "p".repeat(MAX_PROFILE_KEY_BYTES);
        memo.remember(&edge, &site(0), vec![2]);
        assert_eq!(memo.lookup(&edge, &site(0)), Some(vec![2]));
        for n in 0..MAX_MEMO_PROFILES {
            memo.remember(&format!("other{n}"), &site(0), vec![3]);
        }
        assert_eq!(memo.lookup("a", &site(0)), None);
        assert_eq!(memo.lookup("other0", &site(0)), Some(vec![3]));
    }
}
