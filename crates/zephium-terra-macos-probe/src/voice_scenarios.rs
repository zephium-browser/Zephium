//! Platform-neutral voice cases, joined to the assist bench's fixture and policy.
use super::assist_scenarios::{self, Scenario};

pub(super) struct VoiceScenario {
    pub name: &'static str,
    pub assist: &'static str,
    pub request: Option<&'static str>,
    pub expected: &'static [&'static str],
    pub barge: bool,
    pub silence: bool,
}

pub(super) const SCENARIOS: &[VoiceScenario] = &[
    VoiceScenario {
        name: "voice-fact",
        assist: "read-fact",
        request: None,
        expected: &["people", "lakeview"],
        barge: false,
        silence: false,
    },
    VoiceScenario {
        name: "voice-cart",
        assist: "cart-add",
        request: None,
        expected: &["bottle", "cart"],
        barge: false,
        silence: false,
    },
    VoiceScenario {
        name: "voice-no-effect",
        assist: "no-effect",
        request: None,
        expected: &["orbit", "mug"],
        barge: false,
        silence: false,
    },
    VoiceScenario {
        name: "voice-count",
        assist: "count-only",
        request: Some("add one serving to this recipe, then tell me how many it serves"),
        expected: &["serving", "recipe"],
        barge: false,
        silence: false,
    },
    VoiceScenario {
        name: "voice-confirm",
        assist: "contact-form",
        request: None,
        expected: &["contact", "billing", "invoice"],
        barge: false,
        silence: false,
    },
    VoiceScenario {
        name: "voice-barge",
        assist: "read-fact",
        request: Some("give me a three sentence summary of this page"),
        expected: &["summary", "page"],
        barge: true,
        silence: false,
    },
    VoiceScenario {
        name: "voice-barge-ask",
        assist: "read-fact",
        request: Some("give me a three sentence summary of this page"),
        expected: &["summary", "page"],
        barge: true,
        silence: false,
    },
    VoiceScenario {
        name: "voice-silence",
        assist: "read-fact",
        request: None,
        expected: &[],
        barge: false,
        silence: true,
    },
];

impl VoiceScenario {
    pub fn second_text(&self) -> &'static str {
        if self.name == "voice-barge-ask" {
            "what is the title of this page"
        } else {
            "stop"
        }
    }
    pub fn second_expected(&self) -> &'static [&'static str] {
        if self.name == "voice-barge-ask" {
            &["title", "page"]
        } else {
            &["stop"]
        }
    }
    pub fn fixture(&self) -> Option<&'static Scenario> {
        assist_scenarios::find(self.assist)
    }
    pub fn text(&self) -> Option<&'static str> {
        self.request.or_else(|| self.fixture().map(|s| s.request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_scenarios_reuse_only_loopback_fixtures() {
        let mut names = std::collections::BTreeSet::new();
        for scenario in SCENARIOS {
            assert!(names.insert(scenario.name));
            assert!(!scenario.fixture().unwrap().live);
            assert!(scenario.text().is_some());
        }
        assert_eq!(names.len(), 8);
    }
}
