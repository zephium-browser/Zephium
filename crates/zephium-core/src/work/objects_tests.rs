use super::*;
use crate::work::{
    artifact::{WorkArtifactDataV1 as Data, WorkArtifactField as F, *},
    model::{WorkModelProvider, WorkModelRef, WorkModelWire},
    parts::*,
    runtime::*,
    search::*,
    *,
};
use serde_json::json;

fn data(value: serde_json::Value) -> Data {
    serde_json::from_value(value).expect("the wire shape parses")
}
fn fault(value: serde_json::Value, evidence: usize) -> Option<WorkObjectFault> {
    data(value).lead_fault(evidence)
}
fn field(value: serde_json::Value) -> Option<F> {
    fault(value, 0).map(|fault| fault.field)
}

#[test]
fn every_kind_round_trips_its_wire_shape() {
    let shapes = [
        json!({"kind":"reply","headline":"128 PLN","text":"At today's rate, **30 EUR** is about 128 PLN.",
            "figures":[{"label":"Rate","value":"4.27","note":"ECB, 28 Sep"}],"points":["Cards add about 2%"]}),
        json!({"kind":"picks","facet":"flight","items":[{"name":"LOT LO 3","subtitle":"Direct",
            "price":{"display":"$1,240 return","amount":"1240","currency":"USD","was":"$1,480"},
            "facts":[{"label":"Bags","value":"1 checked","kind":"yes"}],
            "rating":{"value":"4.5","max":5,"count":120},"why":"Only direct flight","tags":["direct"],
            "recommended":true,"route":{"from":"WAW","to":"SFO","depart":"10:05","arrive":"13:40",
            "duration":"12h 35m","stops":0,"carrier":"LOT","carrier_host":"lot.com"},"source":0}]}),
        json!({"kind":"plan","steps":[{"when":"Mon 6 Jan","date":"2027-01-04","title":"Fly WAW to SFO","kind":"travel",
            "cost":"$1,240","place":"SFO","pick":{"artifact":"01J00000000000000000000000","index":0}}],
            "total":{"label":"Total","value":"$4,380"},"checkable":true}),
        json!({"kind":"list","style":"messages","items":[{"title":"Reply to Ana about the deck",
            "due":"Today","priority":"high","from":{"host":"slack.com","app":"Slack","who":"Ana",
            "when":"09:12","quote":"Can you look at the deck?","url":"https://app.slack.com/client/T1/C2"}}]}),
        json!({"kind":"sheet","columns":[{"label":"Provider","kind":"entity"},
            {"label":"Price","kind":"money","currency":"USD","best":"min"},
            {"label":"Edge","kind":"yes_no"},{"label":"Rating","kind":"rating"}],
            "rows":[{"cells":["Cloudflare","5","yes","4/5"],"entity":{"logo_host":"cloudflare.com"}},
            {"cells":["Hetzner","4.5","no",""]}],"note":"List prices, September 2026"}),
        json!({"kind":"plot","style":"range","x":{"kind":"category"},
            "y":{"format":"money","currency":"USD","label":"Monthly"},
            "series":[{"name":"Cost","points":[{"x":"Small","y":"550","y2":"900"},{"x":"Large","y":"4000","y2":"9500"}]}],
            "headline":{"label":"Typical","value":"$550–9,500"},"basis":"Published list prices"}),
        json!({"kind":"diff","path":"src/lib.rs","language":"rust","summary":"Guard the empty case",
            "hunks":[{"old_start":10,"new_start":10,"lines":[{"op":"ctx","text":"fn f() {"},
            {"op":"del","text":"    x.unwrap()"},{"op":"add","text":"    x.unwrap_or_default()"}]}]}),
        json!({"kind":"draft","destination":"email","to":"ana@example.com","subject":"The deck",
            "body":"Hi Ana,\n\nThe deck looks good. See [the notes](https://example.com/notes).",
            "target_url":"https://mail.google.com/"}),
        json!({"kind":"project","name":"Lunios","summary":"A Tauri desktop app with a SvelteKit front end",
            "root":"/Users/ana/Dev/Lunios",
            "stack":[{"name":"SvelteKit","version":"2.8","role":"Frontend","host":"svelte.dev","manifest":"package.json"},
            {"name":"Tauri","version":"2","role":"Desktop shell","host":"tauri.app","manifest":"src-tauri/Cargo.toml"}],
            "tree":[{"path":"src","kind":"folder"},{"path":"src/routes","kind":"folder","more":3},
            {"path":"src-tauri","kind":"folder","more":9},{"path":"package.json","kind":"file"}],"more":12,
            "scripts":[{"name":"dev","command":"vite dev","source":"package.json"}],
            "git":{"branch":"main","changed":2,"ahead":1}}),
        json!({"kind":"media","medium":"video","url":"https://www.youtube.com/watch?v=abc","title":"Lecture 1",
            "provider":"youtube","poster":"https://i.ytimg.com/vi/abc/hqdefault.jpg","duration":"1:12:03","start_secs":30}),
        json!({"kind":"subject","noun":"Pull request","reference":"crynta/terax#1337",
            "title":"feat(explorer): support pasting files from OS clipboard",
            "state":{"label":"Open","tone":"active"},"people":[{"label":"Author","value":"drunkardleo-10"}],
            "facts":[{"label":"Changed files","value":"6"},{"label":"Checks","value":"Passing"}],
            "labels":["enhancement"],"summary":"Adds pasting files from the **OS clipboard** with Cmd+V.",
            "url":"https://github.com/crynta/terax/pull/1337"}),
    ];
    for shape in &shapes {
        let parsed = data(shape.clone());
        assert_eq!(serde_json::to_value(&parsed).unwrap(), *shape);
        assert_eq!(parsed.lead_fault(1), None, "{}", parsed.kind_name());
    }
}

#[test]
fn limits_come_back_as_the_field_and_the_item() {
    let long = "界".repeat(limit::PICK_NAME + 1);
    let picks = fault(
        json!({"kind":"picks","facet":"stay","items":[{"name":"Loft"},{"name":long}]}),
        0,
    )
    .unwrap();
    assert_eq!(picks.field, F::PickName);
    assert_eq!(picks.index, Some(1));
    assert!(
        picks
            .describe()
            .starts_with("items[1].name is 201 characters; the limit is 200. Rule: "),
        "{}",
        picks.describe()
    );
    assert_eq!(picks.drop, None);
    let detail = fault(
        json!({"kind":"plan","steps":[{"title":"Fly","kind":"travel"},{"title":"Land","kind":"travel"},
            {"title":"Stay","kind":"stay"},{"title":"Walk","kind":"task","detail":"x".repeat(281)}]}),
        0,
    )
    .unwrap();
    assert!(
        detail
            .describe()
            .starts_with("steps[3].detail is 281 characters; the limit is 280."),
        "{}",
        detail.describe()
    );
    assert_eq!(detail.drop, Some(WorkFaultDrop::Remove("steps[].detail")));
    let fact = fault(
        json!({"kind":"picks","facet":"stay","items":[{"name":"Loft","facts":[
            {"label":"Beds","value":"2","kind":"text"},{"label":"Walk","value":"x".repeat(limit::PICK_FACT_VALUE + 1),"kind":"text"}]}]}),
        0,
    )
    .unwrap();
    assert_eq!(fact.fill(fact.path.unwrap()), "items[0].facts[1].value");
    let was = fault(
        json!({"kind":"picks","facet":"product","items":[{"name":"Falcon",
            "price":{"display":"$79.99","was":"x".repeat(limit::PICK_PRICE + 1)}}]}),
        0,
    )
    .unwrap();
    assert!(was
        .describe()
        .starts_with("items[0].price.was is 65 characters; the limit is 64."));
    assert_eq!(was.drop, Some(WorkFaultDrop::Remove("items[].price.was")));
    assert_eq!(fact.drop, Some(WorkFaultDrop::Remove("items[].facts[]")));
    let cells = |first: &str, second: &str| {
        fault(
            json!({"kind":"sheet","columns":[{"label":"Name","kind":"text"},{"label":"Note","kind":"text"}],
                "rows":[{"cells":[first, second]}]}),
            0,
        )
        .unwrap()
    };
    assert_eq!(
        cells("Hetzner", &"x".repeat(limit::CELL_TEXT + 1)).drop,
        Some(WorkFaultDrop::Empty("rows[].cells[]"))
    );
    assert_eq!(cells(&"x".repeat(limit::CELL_TEXT + 1), "ok").drop, None);
    let points = fault(
        json!({"kind":"reply","headline":"H","text":"T","points":["a","b","c","d","e","f"]}),
        0,
    )
    .unwrap();
    assert!(points
        .describe()
        .starts_with("points has 6 items; the limit is 5."));
    assert_eq!(points.drop, Some(WorkFaultDrop::Keep("points")));
    assert_eq!(
        field(
            json!({"kind":"picks","facet":"stay","items":[{"name":"A","recommended":true},{"name":"B","recommended":true}]})
        ),
        Some(F::PickRecommended)
    );
    assert_eq!(
        field(json!({"kind":"reply","headline":"H","text":"See [this](https://x.test)"})),
        Some(F::ReplyMarkup)
    );
    assert_eq!(
        field(json!({"kind":"reply","headline":"H","text":"- one\n- two"})),
        Some(F::ReplyMarkup)
    );
    assert_eq!(
        field(json!({"kind":"reply","headline":"H","text":"x".repeat(481)})),
        Some(F::ReplyText)
    );
    assert_eq!(
        field(
            json!({"kind":"sheet","columns":[{"label":"Note","kind":"text"}],
            "rows":[{"cells":["界".repeat(limit::CELL_TEXT + 1)]}]})
        ),
        Some(F::SheetCell)
    );
    assert_eq!(
        field(
            json!({"kind":"sheet","columns":[{"label":"Price","kind":"money"}],"rows":[{"cells":["5"]}]})
        ),
        Some(F::SheetColumn)
    );
    assert_eq!(
        field(
            json!({"kind":"sheet","columns":[{"label":"Ok","kind":"yes_no"}],"rows":[{"cells":["maybe"]}]})
        ),
        Some(F::SheetCell)
    );
    assert_eq!(
        field(
            json!({"kind":"plot","style":"bar","x":{"kind":"category"},"y":{"format":"number"},
            "series":[{"name":"S","points":[{"x":"a","y":"3"},{"x":"b","y":"3"}]}],"basis":"Counted"})
        ),
        Some(F::PlotValues)
    );
    assert_eq!(
        field(
            json!({"kind":"plot","style":"bar","x":{"kind":"category"},"y":{"format":"number"},
            "series":[{"name":"S","points":[{"x":"a"}]}],"basis":"Counted"})
        ),
        Some(F::PlotValues)
    );
    assert_eq!(
        field(
            json!({"kind":"media","medium":"video","url":"https://evil.test/v","provider":"youtube"})
        ),
        Some(F::MediaProvider)
    );
    assert_eq!(
        field(json!({"kind":"media","medium":"image","url":"http://example.com/a.jpg"})),
        Some(F::MediaUrl)
    );
    assert_eq!(
        field(json!({"kind":"draft","destination":"slack","subject":"Hi","body":"Hello"})),
        Some(F::DraftSubject)
    );
    assert_eq!(
        field(json!({"kind":"plan","steps":[{"title":"Go","kind":"travel","source":2}]})),
        Some(F::ItemSource)
    );
    assert_eq!(
        fault(
            json!({"kind":"plan","steps":[{"title":"Go","kind":"travel","source":0}]}),
            1
        ),
        None
    );
}

#[test]
fn a_lead_run_refuses_legacy_kinds_and_holds_diagrams_to_its_limits() {
    assert_eq!(
        field(json!({"kind":"table","columns":["a"],"rows":[["b"]]})),
        Some(F::LegacyKind)
    );
    assert_eq!(
        field(json!({"kind":"answer","markdown":"Hello"})),
        Some(F::LegacyKind)
    );
    let node =
        |id: usize, name: &str| json!({"id": format!("n{id}"), "name": name, "kind": "service"});
    let nodes: Vec<_> = (0..25).map(|i| node(i, "Service")).collect();
    assert_eq!(
        field(json!({"kind":"diagram","nodes":nodes,"edges":[]})),
        Some(F::LeadDiagramSize)
    );
    let named = fault(
        json!({"kind":"diagram","nodes":[node(0, "API"), node(1, "A name longer than twenty-eight")],"edges":[]}),
        0,
    )
    .unwrap();
    assert_eq!((named.field, named.index), (F::LeadDiagramNode, Some(1)));
    // Saved diagrams keep their own, wider limits.
    let saved = data(
        json!({"kind":"diagram","nodes":[node(0, "A name longer than twenty-eight")],"edges":[]}),
    );
    assert_eq!(saved.validate(0), Ok(()));
}

#[test]
fn observed_pictures_and_links_need_a_source() {
    let artifact = |data: Data, evidence: Vec<WorkEvidenceLink>| WorkArtifactV1 {
        version: 1,
        id: 1.into(),
        execution: 2.into(),
        node: 3.into(),
        attempt: 4.into(),
        output: "result".into(),
        title: "Homes".into(),
        data,
        evidence,
        review: WorkOutputReview::SourceMappedNeedsReview,
        presentation: WorkArtifactPresentationV1::Automatic,
        general_knowledge: true,
        revises: None,
        part: None,
    };
    let pictured = data(
        json!({"kind":"picks","facet":"stay","items":[{"name":"Loft",
        "image_candidates":["https://a0.muscache.com/im/pictures/1.jpg"]}]}),
    );
    assert_eq!(
        artifact(pictured.clone(), vec![]).validate(),
        Err(WorkError::Invalid)
    );
    let mut cited = artifact(
        pictured,
        vec![WorkEvidenceLink {
            extraction_id: 9.into(),
            source_id: 1,
        }],
    );
    cited.general_knowledge = false;
    assert_eq!(cited.validate(), Ok(()));
    let known = data(
        json!({"kind":"reply","headline":"Frank Herbert","text":"He wrote *Dune*, published in 1965."}),
    );
    assert_eq!(artifact(known, vec![]).validate(), Ok(()));
}

fn lead_execution() -> (WorkPlanRevision, WorkExecutionFact, WorkRevision) {
    let plan = WorkPlanRevision {
        context: None,
        author: WorkAuthor::User,
        revision: WorkRevision::new(2).unwrap(),
        basis_revision: WorkRevision::INITIAL,
        draft: WorkPlanDraft {
            id: 2.into(),
            nodes: vec![WorkPlanNode {
                id: 3.into(),
                objective: "Plan the trip".into(),
                dependencies: vec![],
                outputs: vec![WorkExpectedOutput {
                    name: "result".into(),
                    description: "The result".into(),
                    review: WorkOutputReview::SourceMappedNeedsReview,
                }],
            }],
        },
    };
    let limits = WorkExecutionLimits {
        model_tokens: 1_000_000,
        cost_micro_usd: 3_000_000,
        operations: 256,
        timeout_seconds: 1800,
        max_workers: 4,
    };
    let grant = WorkAgentGrantV1 {
        no_usage_charge: false,
        draft: None,
        provider: WorkSearchProvider::OpenAi,
        model: PUBLIC_SEARCH_MODEL.into(),
        max_turns: 200,
        max_steps: 255,
        browse_hops: 4,
        folders: vec![],
        accounts: vec![],
        private: false,
        lead: Some(WorkModelRef {
            provider: WorkModelProvider::Anthropic,
            wire: WorkModelWire::AnthropicMessages,
            model: "claude-sonnet-5".into(),
        }),
        skill: None,
        search_fallback: None,
    };
    grant.validate().unwrap();
    assert!(WorkAgentGrantV1 {
        no_usage_charge: false,
        lead: None,
        ..grant.clone()
    }
    .validate()
    .is_err());
    let spec = WorkExecutionSpec::agent(&plan, limits, grant).unwrap();
    let fact = WorkExecutionFact {
        authorization: WorkExecutionAuthorization::UserDirectedAgent,
        id: 7.into(),
        approved_revision: WorkRevision::new(3).unwrap(),
        spec,
        status: WorkExecutionStatus::Running,
        attempts: vec![WorkAttemptFact {
            id: 500.into(),
            node: 3.into(),
            status: WorkAttemptStatus::Running,
            usage: None,
        }],
        artifacts: vec![],
        provider_evidence: vec![],
        file_evidence: vec![],
        command_evidence: vec![],
        folder_approvals: vec![],
        user_artifacts: vec![],
        intervention: None,
        steps: vec![],
        accounts: vec![],
        parts: vec![],
        inputs: vec![],
        title: None,
    };
    (plan, fact, WorkRevision::new(9).unwrap())
}

fn part(id: u128, state: WorkPartStateV1) -> WorkPartFactV1 {
    WorkPartFactV1 {
        id: id.into(),
        title: "Stay".into(),
        helper: WorkHelperV1::Browser,
        service: Some(WorkPartServiceV1 {
            host: Some("airbnb.com".into()),
            connection: None,
        }),
        goal: "Three homes near the YC office for the batch dates".into(),
        state,
        started_ms: (state != WorkPartStateV1::Planned).then(|| "1790000000000".into()),
        ended_ms: state.terminal().then(|| "1790000100000".into()),
        summary: state.terminal().then(|| "3 homes".into()),
        need: None,
    }
}

#[test]
fn parts_and_inputs_are_lead_facts_that_steps_and_objects_name() {
    let (plan, mut fact, revision) = lead_execution();
    fact.parts.push(part(60, WorkPartStateV1::Running));
    fact.inputs.push(WorkInputFactV1 {
        kind: WorkInputKindV1::Skill,
        label: "Trip planning".into(),
        count: None,
        reference: Some("trip-planning".into()),
    });
    let step = WorkStepFact {
        model_fault: None,
        id: 1.into(),
        turn: 1,
        kind: WorkStepKindV1::Search {
            query: "homes near YC".into(),
        },
        status: WorkStepStatus::Running,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
        part: Some(60.into()),
    };
    fact.steps.push(step.clone());
    fact.validate(&plan, revision).unwrap();
    let mut orphan = fact.clone();
    orphan.steps[0].part = Some(61.into());
    assert!(orphan.validate(&plan, revision).is_err());
    let mut legacy = fact.clone();
    legacy.spec.nodes[0].capability = WorkCapability::Agent {
        grant: WorkAgentGrantV1 {
            no_usage_charge: false,
            lead: None,
            max_turns: 8,
            max_steps: 24,
            ..fact.agent_grant().unwrap().clone()
        },
    };
    assert!(legacy.validate(&plan, revision).is_err());
    for broken in [
        WorkPartFactV1 {
            title: "A title longer than twenty-four".into(),
            ..part(62, WorkPartStateV1::Running)
        },
        WorkPartFactV1 {
            ended_ms: None,
            ..part(62, WorkPartStateV1::Done)
        },
        WorkPartFactV1 {
            service: Some(WorkPartServiceV1 {
                host: Some("https://airbnb.com".into()),
                connection: None,
            }),
            ..part(62, WorkPartStateV1::Running)
        },
    ] {
        assert!(broken.validate().is_err());
    }
    let wire = serde_json::to_value(&fact).unwrap();
    assert_eq!(wire["parts"][0]["state"], "running");
    assert_eq!(wire["steps"][0]["part"], fact.parts[0].id.to_string());
    let back: WorkExecutionFact = serde_json::from_value(wire).unwrap();
    assert_eq!(back, fact);
}

#[test]
fn a_revision_names_one_earlier_object_once() {
    let (plan, mut fact, revision) = lead_execution();
    let object = |id: u128, revises: Option<u128>| WorkArtifactV1 {
        version: 1,
        id: id.into(),
        execution: fact.id,
        node: 3.into(),
        attempt: 500.into(),
        output: "result".into(),
        title: "Your trip".into(),
        data: data(json!({"kind":"reply","headline":"Your trip","text":"Six nights in SoMa."})),
        evidence: vec![],
        review: WorkOutputReview::SourceMappedNeedsReview,
        presentation: WorkArtifactPresentationV1::Automatic,
        general_knowledge: true,
        revises: revises.map(Into::into),
        part: None,
    };
    let publish = |id: u128, artifacts: Vec<u128>| WorkStepFact {
        model_fault: None,
        id: id.into(),
        turn: 1,
        kind: WorkStepKindV1::Publish,
        status: WorkStepStatus::Succeeded,
        usage: None,
        artifacts: artifacts.into_iter().map(Into::into).collect(),
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
        part: None,
    };
    fact.artifacts = vec![object(10, None), object(11, Some(10))];
    fact.steps = vec![publish(1, vec![10]), publish(2, vec![11])];
    fact.validate(&plan, revision).unwrap();
    let mut twice = fact.clone();
    twice.artifacts.push(object(12, Some(10)));
    twice.steps.push(publish(3, vec![12]));
    assert!(twice.validate(&plan, revision).is_err());
    let mut forward = fact.clone();
    forward.artifacts[0].revises = Some(11.into());
    forward.artifacts[1].revises = None;
    assert!(forward.validate(&plan, revision).is_err());
    let mut own = fact.clone();
    own.artifacts[1].revises = Some(11.into());
    assert!(own.validate(&plan, revision).is_err());
    // An object of an earlier run is revised by name; the store checks it exists.
    let mut earlier = fact.clone();
    earlier.artifacts[1].revises = Some(99.into());
    earlier.validate(&plan, revision).unwrap();
}

#[test]
fn saved_artifacts_without_the_new_fields_still_load() {
    let saved = json!({
        "version":1,"id":"01J00000000000000000000001","execution":"01J00000000000000000000002",
        "node":"01J00000000000000000000003","attempt":"01J00000000000000000000004",
        "output":"comparison","title":"Answer","data":{"kind":"answer","markdown":"Hello"},
        "evidence":[],"review":"source_mapped_needs_review","presentation":"automatic",
        "general_knowledge":true
    });
    let artifact: WorkArtifactV1 = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(artifact.revises, None);
    assert_eq!(artifact.part, None);
    artifact.validate().unwrap();
    assert_eq!(serde_json::to_value(&artifact).unwrap(), saved);
}

#[test]
fn a_connection_call_is_a_settled_step_with_closed_fields() {
    let call = |url: Option<&str>| WorkStepFact {
        model_fault: None,
        id: 1.into(),
        turn: 1,
        kind: WorkStepKindV1::Call {
            call: Box::new(WorkConnectionCallV1 {
                service: "github".into(),
                tool: "github_issue".into(),
                verb: "issue".into(),
                target: Some("#123".into()),
                title: Some("Totals skip a night".into()),
                count: None,
                url: url.map(str::to_owned),
            }),
        },
        status: WorkStepStatus::Succeeded,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: Some("Read issue #123".into()),
        measurements: None,
        local: None,
        account: None,
        part: None,
    };
    call(Some("https://github.com/octo/app/issues/123"))
        .validate()
        .unwrap();
    assert!(call(Some("http://github.com/x")).validate().is_err());
    let mut running = call(None);
    running.status = WorkStepStatus::Running;
    assert!(running.validate().is_err());
    let wire = serde_json::to_value(call(None).kind).unwrap();
    assert_eq!(wire["kind"], "call");
    assert_eq!(wire["call"]["verb"], "issue");
}

#[test]
fn a_project_keeps_its_tree_shallow_and_ordered() {
    let project = |tree: serde_json::Value| {
        json!({"kind":"project","name":"Lunios","summary":"A Tauri app","root":"/Users/ana/Dev/Lunios",
            "tree": tree})
    };
    assert_eq!(
        field(project(
            json!([{"path":"src","kind":"folder"},{"path":"src/lib.rs","kind":"file"}])
        )),
        None
    );
    for broken in [
        json!([]),
        json!([{"path":"src/lib.rs","kind":"file"}]),
        json!([{"path":"a","kind":"folder"},{"path":"a/b","kind":"folder"},{"path":"a/b/c","kind":"folder"},
            {"path":"a/b/c/d","kind":"file"}]),
        json!([{"path":"../etc","kind":"folder"}]),
        json!([{"path":"/src","kind":"folder"}]),
        json!([{"path":"README.md","kind":"file","more":3}]),
        json!([{"path":"src","kind":"folder"},{"path":"src","kind":"folder"}]),
        json!([{"path":"main.rs","kind":"file"},{"path":"main.rs/x","kind":"file"}]),
    ] {
        assert_eq!(field(project(broken)), Some(F::ProjectTree));
    }
    let mut relative_root = project(json!([{"path":"src","kind":"folder"}]));
    relative_root["root"] = json!("Dev/Lunios");
    assert_eq!(field(relative_root), Some(F::ProjectRoot));
    let mut twice = project(json!([{"path":"src","kind":"folder"}]));
    twice["stack"] = json!([{"name":"Rust"},{"name":"rust"}]);
    assert_eq!(field(twice), Some(F::ProjectStack));
    let mut url_host = project(json!([{"path":"src","kind":"folder"}]));
    url_host["stack"] = json!([{"name":"Svelte","host":"https://svelte.dev"}]);
    assert_eq!(field(url_host), Some(F::ProjectStack));
}

#[test]
fn a_part_needs_something_only_once_it_cannot_go_on() {
    let with = |state, need| WorkPartFactV1 {
        need: Some(need),
        ..part(70, state)
    };
    let sign_in = || WorkPartNeedV1::SignIn {
        host: "slack.com".into(),
    };
    assert!(with(WorkPartStateV1::Failed, sign_in()).validate().is_ok());
    assert!(with(WorkPartStateV1::Waiting, sign_in()).validate().is_ok());
    let check = WorkPartNeedV1::Check {
        host: "state.gov".into(),
    };
    assert!(with(WorkPartStateV1::Waiting, check.clone())
        .validate()
        .is_ok());
    assert_eq!(
        serde_json::to_value(&check).unwrap(),
        json!({"kind":"check", "host":"state.gov"})
    );
    assert_eq!(
        serde_json::from_value::<WorkPartNeedV1>(serde_json::to_value(&check).unwrap()).unwrap(),
        check
    );
    assert!(with(
        WorkPartStateV1::Stopped,
        WorkPartNeedV1::Retry {
            host: None,
            reason: None
        }
    )
    .validate()
    .is_ok());
    for state in [
        WorkPartStateV1::Planned,
        WorkPartStateV1::Running,
        WorkPartStateV1::Done,
    ] {
        assert!(with(state, sign_in()).validate().is_err());
    }
    for need in [
        WorkPartNeedV1::SignIn {
            host: "https://slack.com".into(),
        },
        WorkPartNeedV1::Check {
            host: "https://state.gov/check?token=private".into(),
        },
        WorkPartNeedV1::AllowFolder {
            path: "Dev/Lunios".into(),
        },
        WorkPartNeedV1::UseConnection {
            connection: String::new(),
            reason: None,
        },
        WorkPartNeedV1::Retry {
            host: Some("localhost".into()),
            reason: Some(WorkPartReasonV1::NoAnswer),
        },
    ] {
        assert!(with(WorkPartStateV1::Failed, need).validate().is_err());
    }
    let wire = serde_json::to_value(with(
        WorkPartStateV1::Failed,
        WorkPartNeedV1::AllowFolder {
            path: "/Users/ana/Dev/Lunios".into(),
        },
    ))
    .unwrap();
    assert_eq!(
        wire["need"],
        json!({"kind": "allow_folder", "path": "/Users/ana/Dev/Lunios"})
    );
    let read = serde_json::to_value(with(
        WorkPartStateV1::Failed,
        WorkPartNeedV1::Retry {
            host: Some("kayak.com".into()),
            reason: Some(WorkPartReasonV1::CouldntRead),
        },
    ))
    .unwrap();
    assert_eq!(
        read["need"],
        json!({"kind": "retry", "host": "kayak.com", "reason": "couldnt_read"})
    );
}

#[test]
fn a_folder_question_names_its_folder() {
    let ask = |local: Option<WorkLocalStepV1>| WorkStepFact {
        model_fault: None,
        id: 1.into(),
        turn: 1,
        kind: WorkStepKindV1::Ask {
            prompt: "Read Lunios?".into(),
            options: vec!["Allow for this work".into(), "Not now".into()],
            answer: None,
            purpose: Some(WorkAskPurposeV1::Folder),
        },
        status: WorkStepStatus::Running,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: local.map(Box::new),
        account: None,
        part: None,
    };
    let folder = |path: &str| WorkLocalStepV1 {
        folder: Some(path.into()),
        ..Default::default()
    };
    assert!(ask(Some(folder("/Users/ana/Dev/Lunios")))
        .validate()
        .is_ok());
    assert!(ask(None).validate().is_err());
    assert!(ask(Some(WorkLocalStepV1::default())).validate().is_ok());
    assert!(WorkPartNeedV1::ChooseFolder.validate().is_ok());
    assert!(ask(Some(folder("Dev/Lunios"))).validate().is_err());
    let mut question = ask(Some(folder("/Users/ana/Dev/Lunios")));
    question.kind = WorkStepKindV1::Ask {
        prompt: "Which dates?".into(),
        options: vec![],
        answer: None,
        purpose: Some(WorkAskPurposeV1::Question),
    };
    assert!(question.validate().is_err());
}

#[test]
fn a_first_run_names_its_work_from_its_finish() {
    let (plan, mut fact, revision) = lead_execution();
    let finish = |title: Option<&str>| WorkStepFact {
        model_fault: None,
        id: 9.into(),
        turn: 2,
        kind: WorkStepKindV1::Finish {
            followups: vec![],
            title: title.map(str::to_owned),
        },
        status: WorkStepStatus::Succeeded,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
        part: None,
    };
    for refused in [
        "Plan a trip from Warsaw to San Francisco",
        " YC trip",
        "",
        "A name that runs well past forty-eight characters",
    ] {
        assert!(finish(Some(refused)).validate().is_err(), "{refused}");
    }
    fact.steps.push(finish(Some("YC trip from Warsaw")));
    assert!(fact.validate(&plan, revision).is_err());
    fact.refresh_accounts();
    assert_eq!(fact.title.as_deref(), Some("YC trip from Warsaw"));
    fact.validate(&plan, revision).unwrap();
    let wire = serde_json::to_value(&fact).unwrap();
    assert_eq!(wire["title"], "YC trip from Warsaw");
    assert_eq!(wire["steps"][0]["kind"]["title"], "YC trip from Warsaw");
    let (_, mut unnamed, _) = lead_execution();
    unnamed.steps.push(finish(None));
    unnamed.refresh_accounts();
    assert_eq!(unnamed.title, None);
    assert!(serde_json::to_value(&unnamed)
        .unwrap()
        .get("title")
        .is_none());
}

#[test]
fn a_subject_needs_its_noun_title_and_address_and_sheds_what_overflows() {
    let base = |extra: serde_json::Value| {
        let mut value = json!({"kind":"subject","noun":"Issue","title":"Rerun workflows like Playwright",
            "url":"https://github.com/zephium-browser/Zephium/issues/103"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        value
    };
    assert_eq!(field(base(json!({}))), None);
    assert_eq!(
        field(base(json!({"url":"http://localhost/x"}))),
        Some(F::SubjectUrl)
    );
    assert_eq!(
        field(base(json!({"title":"x".repeat(161)}))),
        Some(F::SubjectTitle)
    );
    assert_eq!(
        field(base(json!({"summary":"# Heading"}))),
        Some(F::SubjectSummary)
    );
    let people = fault(
        base(
            json!({"people":[{"label":"Author","value":"a"},{"label":"B","value":"b"},
        {"label":"C","value":"c"},{"label":"D","value":"d"},{"label":"E","value":"e"}]}),
        ),
        0,
    )
    .unwrap();
    assert_eq!(people.field, F::SubjectPeople);
    assert_eq!(people.drop, Some(WorkFaultDrop::Keep("people")));
}

#[test]
fn a_plan_step_date_is_a_real_day_or_goes() {
    let step =
        |date: &str| json!({"kind":"plan","steps":[{"date":date,"title":"Fly","kind":"travel"}]});
    assert_eq!(field(step("2028-02-29")), None);
    for wrong in ["2027-02-29", "2027-13-01", "9 Oct", "2027-1-04"] {
        let fault = fault(step(wrong), 0).unwrap();
        assert_eq!(fault.field, F::PlanStepDate, "{wrong}");
        assert_eq!(fault.drop, Some(WorkFaultDrop::Remove("steps[].date")));
    }
}

#[test]
fn the_persons_own_table_list_or_plan_may_stand_empty() {
    let empty = [
        json!({"kind":"plan","steps":[]}),
        json!({"kind":"list","style":"todo","items":[]}),
        json!({"kind":"sheet","columns":[{"label":"Name","kind":"text"}],"rows":[]}),
    ];
    for shape in empty {
        let data = data(shape);
        assert!(data.validate(0).is_err(), "{}", data.kind_name());
        assert!(data.validate_owned(0).is_ok(), "{}", data.kind_name());
    }
}

#[test]
fn extracted_record_limits_count_unicode_characters_without_changing_values() {
    let shape = json!({"kind":"picks","facet":"product","items":[{
        "name":"界".repeat(200),"price":{"display":"界".repeat(64)},
        "facts":[{"label":"Stock","value":"界".repeat(120),"kind":"text"}]
    }]});
    let parsed = data(shape.clone());
    assert!(parsed.validate(0).is_ok());
    assert_eq!(serde_json::to_value(parsed).unwrap(), shape);
    let sheet = json!({"kind":"sheet","columns":[{"label":"Value","kind":"text"}],"rows":[{"cells":["界".repeat(200)]}]});
    assert!(data(sheet).validate(0).is_ok());
    for (path, size) in [
        ("/items/0/name", 201),
        ("/items/0/price/display", 65),
        ("/items/0/facts/0/value", 121),
    ] {
        let mut too_long = shape.clone();
        *too_long.pointer_mut(path).unwrap() = json!("界".repeat(size));
        assert!(data(too_long).validate(0).is_err());
    }
}
