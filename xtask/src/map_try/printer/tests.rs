use super::*;
use zephium_map_driver::wire::{Confidence, Network, Official};

fn entry(id: &str, label: &str, path: &str) -> Entry {
    Entry {
        menu_only: false,
        id: id.into(),
        tier: Tier::Primary,
        label: label.into(),
        role: None,
        group: None,
        off_site: false,
        open: false,
        host: None,
        path: path.into(),
        read: None,
        children: Vec::new(),
        more: false,
    }
}

fn place(title: &str, host: &str, sections: Vec<Entry>) -> Place {
    Place {
        title: title.into(),
        site: host.into(),
        host: host.into(),
        sections,
        focus: None,
        social: Vec::new(),
        notes: Default::default(),
    }
}

fn changed(slot: u8, place: Place, complete: bool) -> Change {
    Change::Place {
        seq: 1,
        slot,
        place,
        confidence: Confidence::High,
        stale: false,
        complete,
    }
}

#[test]
fn comparison_sides_disambiguate_an_open_label() {
    let mut printer = Printer::default();
    for (slot, id) in [(0, "first"), (1, "second")] {
        let mut rows = vec![entry(id, "Pricing", "/pricing")];
        if slot == 1 {
            rows.push(entry("docs", "Docs", "/docs"));
        }
        printer.lines(
            &changed(slot, place("Site", "example.com", rows), true),
            Duration::ZERO,
            Counts::default(),
        );
    }
    assert_eq!(printer.section("Pricing"), Err("ambiguous label"));
    assert_eq!(printer.section("left::Pricing"), Ok((0, "first")));
    assert_eq!(printer.section("right::Pricing"), Ok((1, "second")));
    assert_eq!(printer.section("Docs"), Ok((1, "docs")));
    assert_eq!(printer.section("left::Docs"), Err("unknown label"));
}

#[test]
fn place_stream_golden() {
    let first = Change::FirstLayer {
        seq: 0,
        official: Some(Official {
            title: "Stripe".into(),
            site: "stripe.com".into(),
        }),
        social: vec![Network::GitHub],
        organic: 1,
        also: Vec::new(),
    };
    let mut place = place(
        "Stripe",
        "stripe.com",
        vec![
            Entry {
                tier: Tier::Secondary,
                ..entry("about", "About", "/about")
            },
            Entry {
                children: vec![entry("tiers", "Tiers", "/pricing/tiers")],
                more: true,
                ..entry("pricing", "Pricing", "/pricing")
            },
            Entry {
                off_site: true,
                ..entry("docs", "Docs", "/docs")
            },
        ],
    );
    place.focus = Some("tiers".into());
    let update = changed(0, place, true);
    let end = Change::End {
        seq: 2,
        outcome: Outcome::Done,
        elapsed_ms: 650,
    };
    let counts = Counts {
        fetches: 2,
        bytes: 512,
        render_unavailable: 1,
        ..Counts::default()
    };
    let mut printer = Printer::default();
    assert_eq!(
        intro("Stripe Pricing"),
        "Stand-in results page for Stripe Pricing"
    );
    let mut lines = Vec::new();
    for change in [first, update, end] {
        lines.extend(printer.lines(&change, Duration::ZERO, counts));
    }
    assert_eq!(
        lines.join("\n"),
        concat!(
            "First layer  1 organic  official Stripe (stripe.com)  social GitHub\n",
            "Place 1  Stripe (stripe.com)\n",
            "  Pricing  /pricing [more]\n",
            "    * Tiers  /pricing/tiers\n",
            "  Docs  /docs [off-site]\n",
            "  --- Secondary ---\n",
            "  About  /about\n",
            "Done  650ms  reads 2  data 0.0 MB  renders wanted 1"
        )
    );
}

#[test]
fn comparison_uses_current_labels_for_each_side_including_throttled_updates() {
    let mut printer = Printer::default();
    let print = |printer: &mut Printer, change: &Change| {
        printer.lines(change, Duration::ZERO, Counts::default())
    };
    let left = changed(
        0,
        place(
            "Figma",
            "figma.com",
            vec![entry("price", "Plans", "/pricing")],
        ),
        false,
    );
    let right = changed(
        1,
        place(
            "Sketch",
            "sketch.com",
            vec![
                entry("price", "Pricing", "/pricing"),
                Entry {
                    children: vec![entry("help", "Tutorials", "/help/learn")],
                    ..entry("docs", "Documentation", "/docs")
                },
            ],
        ),
        false,
    );
    let updated_left = changed(
        0,
        place(
            "Figma",
            "figma.com",
            vec![entry("price", "Plans and Pricing", "/pricing")],
        ),
        false,
    );
    assert!(!print(&mut printer, &left).is_empty());
    assert!(!print(&mut printer, &right).is_empty());
    assert!(print(&mut printer, &updated_left).is_empty());
    let compare = Change::Compare {
        seq: 3,
        rows: vec![
            Row {
                role: None,
                label: "Prices".into(),
                left: Some("price".into()),
                right: Some("price".into()),
            },
            Row {
                role: None,
                label: "Learn".into(),
                left: None,
                right: Some("help".into()),
            },
        ],
    };
    assert_eq!(
        print(&mut printer, &compare).join("\n"),
        concat!(
            "Left                                             | Right\n",
            "Plans and Pricing                                | Pricing\n",
            "-                                                | Tutorials",
        )
    );
}

#[test]
fn failed_side_and_refusals_use_plain_words() {
    let failed = Change::PlaceFailed {
        seq: 2,
        slot: 1,
        failure: Failure::Challenge,
    };
    let mut printer = Printer::default();
    assert_eq!(
        printer.lines(&failed, Duration::ZERO, Counts::default()),
        ["Place 2 failed  site requires a challenge"]
    );
    assert_eq!(
        refusal(StartRefusal::NotAMap),
        "Map refused  This query does not describe a place or a comparison."
    );
    assert_eq!(
        refusal(StartRefusal::Invalid),
        "Map refused  The query is invalid or exceeds a limit."
    );
    assert_eq!(
        refusal(StartRefusal::UnsupportedEngine),
        "Map refused  The search engine is unsupported."
    );
    let end = Change::End {
        seq: 3,
        outcome: Outcome::Cancelled,
        elapsed_ms: 4,
    };
    assert_eq!(
        printer.lines(&end, Duration::ZERO, Counts::default()),
        ["Cancelled  4ms  reads 0  data 0.0 MB  renders wanted 0"]
    );
}

#[test]
fn throttles_partial_places_at_the_interval_and_prints_complete_immediately() {
    let mut printer = Printer::default();
    let place = place("Figma", "figma.com", Vec::new());
    let partial = changed(0, place.clone(), false);
    let complete = changed(0, place, true);
    let print = |printer: &mut Printer, change: &Change, time| {
        printer.lines(change, time, Counts::default())
    };
    assert!(!print(&mut printer, &partial, Duration::ZERO).is_empty());
    assert!(print(&mut printer, &partial, PLACE_INTERVAL).is_empty());
    let just_past = PLACE_INTERVAL + Duration::from_nanos(1);
    assert!(!print(&mut printer, &partial, just_past).is_empty());
    assert!(!print(&mut printer, &complete, just_past).is_empty());
}

#[test]
fn clips_to_the_column_limit_and_removes_terminal_controls() {
    assert_eq!(clip(&"a".repeat(COLUMNS), COLUMNS), "a".repeat(COLUMNS));
    assert_eq!(
        clip(&"a".repeat(COLUMNS + 1), COLUMNS),
        format!("{}…", "a".repeat(COLUMNS - 2))
    );
    assert_eq!(clip("Test\x1b[2J\u{202e}\n界", COLUMNS), "Test [2J 界");
    let long = "界".repeat(COLUMNS);
    let place = place(
        &long,
        "figma.com",
        vec![Entry {
            off_site: true,
            more: true,
            ..entry("long", &long, &format!("/{long}"))
        }],
    );
    let lines = place_lines(0, &place);
    assert!(lines.iter().all(|line| columns(line) <= COLUMNS));
    assert!(lines[0].ends_with("… (figma.com)"));
    assert!(lines[1].ends_with("… [off-site] [more]"));
    let mut printer = Printer::default();
    for slot in [0, 1] {
        printer.lines(
            &changed(slot, place.clone(), true),
            Duration::ZERO,
            Counts::default(),
        );
    }
    let line = printer.row(&Row {
        role: None,
        label: "Long".into(),
        left: Some("long".into()),
        right: Some("long".into()),
    });
    assert!(columns(&line) <= COLUMNS);
    assert_eq!(line.matches(ELLIPSIS).count(), 2);
    let end = end_line(
        Outcome::Failed {
            failure: Failure::Challenge,
        },
        u32::MAX,
        Counts {
            fetches: u32::MAX,
            bytes: u64::MAX,
            render_unavailable: u32::MAX,
            ..Counts::default()
        },
    );
    assert!(columns(&end) <= COLUMNS);
    assert!(end.ends_with("renders wanted 4294967295"));
}
