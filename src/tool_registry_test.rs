use crate::test_support::*;
use crate::*;

fn probe(visibility: ToolVisibility) -> ToolSpec {
    ToolSpec {
        name: "visibility_probe",
        description: "test-only",
        parameters: empty_parameters,
        visibility,
        kind: ToolKind::WebSearch,
    }
}

fn payload_names(payload: Option<Value>) -> Vec<String> {
    payload
        .and_then(|payload| payload.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|tool| {
            tool.pointer("/function/name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn builtin_visibility_matches_current_roles() {
    let registry = ToolRegistry::builtin();
    for name in [
        "web_search",
        "research",
        "read_url",
        "propose_feature",
        "feature_status",
    ] {
        assert!(
            registry.find(name, false).is_some(),
            "{name} must be visible to non-owners"
        );
        assert!(
            registry.find(name, true).is_some(),
            "{name} must be visible to the owner"
        );
    }
    for name in [
        "find_stickers",
        "send_sticker",
        "feature_confirmation",
        "inspect_code",
    ] {
        assert!(
            registry.find(name, false).is_none(),
            "{name} must be hidden from non-owners"
        );
        assert!(
            registry.find(name, true).is_some(),
            "{name} must be visible to the owner"
        );
    }
    assert!(registry.find("unknown_tool", true).is_none());
    assert!(registry.find("unknown_tool", false).is_none());

    assert_eq!(
        payload_names(registry.payload(false)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string(),
            "propose_feature".to_string(),
            "feature_status".to_string(),
        ],
    );
    assert_eq!(
        payload_names(registry.payload(true)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string(),
            "propose_feature".to_string(),
            "feature_status".to_string(),
            "feature_confirmation".to_string(),
            "inspect_code".to_string(),
            "find_stickers".to_string(),
            "send_sticker".to_string(),
        ],
    );
    assert_eq!(
        payload_names(registry.payload_for(false, false)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string(),
            "propose_feature".to_string(),
        ],
    );
    assert_eq!(
        payload_names(registry.payload_for(true, false)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string(),
            "propose_feature".to_string(),
            "inspect_code".to_string(),
            "find_stickers".to_string(),
            "send_sticker".to_string(),
        ],
    );
}

#[test]
fn research_mode_hides_side_effect_tools() {
    let registry = ToolRegistry::builtin();
    assert_eq!(
        payload_names(registry.payload_for_mode(true, true, true, false, false)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string()
        ],
    );
    assert_eq!(
        payload_names(registry.payload_for_mode(true, true, true, true, false)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string(),
            "find_stickers".to_string(),
            "send_sticker".to_string(),
        ],
    );
    assert_eq!(
        payload_names(registry.payload_for_mode(true, true, true, false, true)),
        vec![
            "web_search".to_string(),
            "research".to_string(),
            "read_url".to_string(),
            "propose_feature".to_string(),
        ],
    );
}

#[test]
fn builtin_names_match_their_kinds_and_visibility() {
    let registry = ToolRegistry::builtin();
    let expected = [
        (ToolKind::WebSearch, "web_search", ToolVisibility::All),
        (ToolKind::Research, "research", ToolVisibility::All),
        (ToolKind::ReadUrl, "read_url", ToolVisibility::All),
        (
            ToolKind::ProposeFeature,
            "propose_feature",
            ToolVisibility::All,
        ),
        (
            ToolKind::FeatureStatus,
            "feature_status",
            ToolVisibility::All,
        ),
        (
            ToolKind::FeatureConfirmation,
            "feature_confirmation",
            ToolVisibility::Owner,
        ),
        (ToolKind::InspectCode, "inspect_code", ToolVisibility::Owner),
        (
            ToolKind::FindStickers,
            "find_stickers",
            ToolVisibility::Owner,
        ),
        (ToolKind::SendSticker, "send_sticker", ToolVisibility::Owner),
    ];
    let specs: Vec<_> = registry.visible(true).collect();
    assert_eq!(specs.len(), expected.len());
    for (spec, (kind, name, visibility)) in specs.iter().zip(expected) {
        assert_eq!(spec.kind, kind);
        assert_eq!(spec.name, name);
        assert_eq!(spec.visibility, visibility);
        for owner in [false, true] {
            assert_eq!(
                registry.find(spec.name, owner).is_some(),
                visibility.allows(owner),
                "lookup and visibility predicate disagree for {}",
                spec.name,
            );
        }
    }
}

#[test]
fn public_tool_is_visible_to_owner_and_non_owner() {
    let registry = ToolRegistry::builtin().with(probe(ToolVisibility::All));
    for owner in [false, true] {
        assert!(registry.find("visibility_probe", owner).is_some());
        assert!(payload_names(registry.payload(owner))
            .iter()
            .any(|name| name == "visibility_probe"));
    }
}

#[test]
fn owner_only_probe_is_hidden_from_non_owner() {
    let registry = ToolRegistry::builtin().with(probe(ToolVisibility::Owner));
    assert!(registry.find("visibility_probe", false).is_none());
    assert!(!payload_names(registry.payload(false))
        .iter()
        .any(|name| name == "visibility_probe"));
    assert!(registry.find("visibility_probe", true).is_some());
    assert!(payload_names(registry.payload(true))
        .iter()
        .any(|name| name == "visibility_probe"));
}

#[test]
fn empty_registry_omits_tools_payload() {
    let registry = ToolRegistry::default();
    assert!(registry.payload(false).is_none());
    assert!(registry.payload(true).is_none());
    assert!(registry.names(false).is_empty());
    assert!(registry.names(true).is_empty());
}

#[test]
fn duplicate_tool_name_replaces_previous_spec() {
    let registry = ToolRegistry::builtin()
        .with(probe(ToolVisibility::All))
        .with(probe(ToolVisibility::Owner));
    assert_eq!(
        payload_names(registry.payload(true))
            .iter()
            .filter(|name| *name == "visibility_probe")
            .count(),
        1,
    );
    assert!(!payload_names(registry.payload(false))
        .iter()
        .any(|name| name == "visibility_probe"));
}
