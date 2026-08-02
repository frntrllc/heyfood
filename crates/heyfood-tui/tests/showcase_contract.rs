use std::collections::BTreeSet;

use heyfood_core::{
    HouseholdLifecycleV1, HouseholdProfileStateV1, HouseholdRevision, HouseholdScope,
    HouseholdSubjectId, MemberId, ProfileRevision, RelationshipV1,
};
use heyfood_tui::{
    Action, AppModel, Effect, HouseholdAccountBindingDigestV1, HouseholdManagementLoadPurposeV1,
    HouseholdMemberPresentationV1, HouseholdModeGenerationV1, HouseholdMutationKindV1,
    HouseholdPresentationModeV1, RuntimeEvent, SemanticEntry, Speaker, dispatch,
    household_chrome_copy,
};
use serde_json::Value;

fn contract() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/showcase/showcase-contract.v1.json"
    ))
    .expect("showcase contract is valid JSON")
}

#[test]
fn landing_page_inventory_declares_all_twelve_required_stages() {
    let contract = contract();
    assert_eq!(contract["schema_version"], 1);
    let journeys = contract["journeys"].as_array().unwrap();
    assert_eq!(journeys.len(), 3);

    let expected = BTreeSet::from(["dinner-planner", "menu-watch", "voice-meal-log"]);
    let actual = journeys
        .iter()
        .map(|journey| journey["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);

    let mut stage_ids = BTreeSet::new();
    for journey in journeys {
        let stages = journey["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 4);
        for stage in stages {
            let qualified_id = format!(
                "{}:{}",
                journey["id"].as_str().unwrap(),
                stage["id"].as_str().unwrap()
            );
            assert!(stage_ids.insert(qualified_id), "duplicate showcase stage");
            assert!(!stage["requires"].as_array().unwrap().is_empty());
            assert!(!stage["asserts"].as_array().unwrap().is_empty());
        }
    }
    assert_eq!(stage_ids.len(), 12);
    assert_eq!(
        contract["installed_artifact_gate"]["journey_pass_rate"],
        1.0
    );
    assert_eq!(
        contract["installed_artifact_gate"]["placeholder_or_simulated_success_forbidden"],
        true
    );
}

#[test]
fn landing_page_inventory_declares_the_presentation_requirements() {
    let presentation = &contract()["presentation"];
    assert_eq!(
        presentation["composer"],
        "bottom_anchored_multiline_editable_while_streaming"
    );
    assert_eq!(
        presentation["responsive_widths"],
        serde_json::json!([40, 80, 120])
    );
    assert!(
        presentation["footer_controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "Ctrl+C")
    );
    assert!(
        presentation["structured_response"]
            .as_array()
            .unwrap()
            .iter()
            .any(|part| part == "evidence_note")
    );
}

#[derive(Clone, Copy)]
struct LoadEvidence {
    operation_id: heyfood_tui::HouseholdOperationIdV1,
    generation: HouseholdModeGenerationV1,
    digest: HouseholdAccountBindingDigestV1,
    correlation: heyfood_tui::HouseholdReducerCorrelationV1,
    purpose: HouseholdManagementLoadPurposeV1,
}

fn load_evidence(effect: &Effect) -> LoadEvidence {
    let Effect::LoadHouseholdManagementV1 {
        operation_id,
        session_mode_generation,
        expected_account_binding_digest,
        reducer_correlation,
        purpose,
    } = effect
    else {
        panic!("expected a typed household management load, got {effect:?}");
    };
    LoadEvidence {
        operation_id: *operation_id,
        generation: *session_mode_generation,
        digest: *expected_account_binding_digest,
        correlation: *reducer_correlation,
        purpose: *purpose,
    }
}

fn owner() -> HouseholdMemberPresentationV1 {
    HouseholdMemberPresentationV1::new(
        HouseholdSubjectId::self_(),
        "Owner",
        RelationshipV1::Self_,
        HouseholdLifecycleV1::Active,
        HouseholdProfileStateV1::LocalOnly,
        Some(ProfileRevision::new(1).unwrap()),
    )
    .unwrap()
}

fn member(
    id: &str,
    label: &str,
    readiness: HouseholdProfileStateV1,
) -> HouseholdMemberPresentationV1 {
    member_with_relationship(id, label, RelationshipV1::Child, readiness)
}

fn member_with_relationship(
    id: &str,
    label: &str,
    relationship: RelationshipV1,
    readiness: HouseholdProfileStateV1,
) -> HouseholdMemberPresentationV1 {
    HouseholdMemberPresentationV1::new(
        HouseholdSubjectId::member(MemberId::parse_preserved(id).unwrap()),
        label,
        relationship,
        HouseholdLifecycleV1::Active,
        readiness,
        (readiness != HouseholdProfileStateV1::Incomplete)
            .then(|| ProfileRevision::new(1).unwrap()),
    )
    .unwrap()
}

fn submit_text(model: &mut AppModel, text: &str) -> Vec<Effect> {
    let _ = dispatch(model, Action::InsertText(text.into()));
    dispatch(model, Action::Submit)
}

fn bootstrap(
    model: &mut AppModel,
    mode: HouseholdPresentationModeV1,
    members: Vec<HouseholdMemberPresentationV1>,
    scope: HouseholdScope,
) {
    let effects = dispatch(
        model,
        Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
            session_mode_generation: HouseholdModeGenerationV1::new(1).unwrap(),
            mode,
            account_binding_digest: HouseholdAccountBindingDigestV1::from_bytes([7; 32]),
        }),
    );
    assert_eq!(effects.len(), 1);
    let load = load_evidence(&effects[0]);
    assert_eq!(load.purpose, HouseholdManagementLoadPurposeV1::Bootstrap);
    assert!(
        dispatch(
            model,
            Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                operation_id: load.operation_id,
                session_mode_generation: load.generation,
                reducer_correlation: load.correlation,
                purpose: load.purpose,
                account_binding_digest: load.digest,
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: scope,
                members,
            })
        )
        .is_empty()
    );
    assert!(model.household_management_ready());
}

fn answer_complete_profile(model: &mut AppModel) {
    for answer in ["none", "none", "1", "2", "none", "none", "none", "none"] {
        assert_human_household_onboarding_copy(model);
        assert!(submit_text(model, answer).is_empty());
    }
    assert_human_household_onboarding_copy(model);
}

fn assert_human_household_onboarding_copy(model: &AppModel) {
    let text = &model.scrollback.entries().back().unwrap().text;
    for forbidden in [
        "under_13",
        "age_13_17",
        "age_18_plus",
        "range, ID",
        "canonical ID",
        "declared dietary profile",
        "profile-sync consent",
        "remote member sync",
    ] {
        assert!(
            !text.contains(forbidden),
            "human onboarding copy exposed {forbidden:?}: {text}"
        );
    }
}

#[test]
fn native_add_reuses_the_full_profile_flow_and_waits_for_context_apply() {
    let mut model = AppModel::default();
    bootstrap(
        &mut model,
        HouseholdPresentationModeV1::NativeEnabled,
        vec![owner()],
        HouseholdScope::Subject(HouseholdSubjectId::self_()),
    );
    assert_eq!(model.household_chrome_label(), Some("Me"));
    let old_subject_result = "OLD-SUBJECT-RESULT-CANARY";
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: old_subject_result.into(),
        streaming: false,
    });

    let add_load = submit_text(&mut model, "/household add");
    let add = load_evidence(&add_load[0]);
    assert_eq!(add.purpose, HouseholdManagementLoadPurposeV1::AddMember);
    let _ = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id: add.operation_id,
            session_mode_generation: add.generation,
            reducer_correlation: add.correlation,
            purpose: add.purpose,
            account_binding_digest: add.digest,
            household_revision: HouseholdRevision::new(1).unwrap(),
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            members: vec![owner()],
        }),
    );

    let relationship_prompt = &model.scrollback.entries().back().unwrap().text;
    assert!(
        relationship_prompt
            .contains("Type a number from `1` to `8` or the relationship name, then press Enter.")
    );
    assert!(submit_text(&mut model, "/household add").is_empty());
    assert!(
        model
            .scrollback
            .entries()
            .back()
            .unwrap()
            .text
            .contains("Household member setup is already open")
    );

    // Registered slash commands are parsed before member onboarding answers.
    assert!(submit_text(&mut model, "/for me").is_empty());
    assert!(submit_text(&mut model, "4").is_empty());
    let secret_label = "D3-CANARY-MEMBER-7Q";
    assert!(submit_text(&mut model, secret_label).is_empty());
    let age_prompt = &model.scrollback.entries().back().unwrap().text;
    assert!(age_prompt.contains("age group"));
    assert!(age_prompt.contains("1. Under 13"));
    assert!(age_prompt.contains("2. 13–17"));
    assert!(age_prompt.contains("3. 18 or older"));
    assert!(age_prompt.contains("4. Not sure"));
    assert_human_household_onboarding_copy(&model);
    assert!(submit_text(&mut model, "18 or older").is_empty());
    answer_complete_profile(&mut model);
    let create = submit_text(&mut model, "save");
    assert_eq!(create.len(), 1);
    let (binding, bounded_member_draft, onboarding_profile_input) = match &create[0] {
        Effect::CreateMemberWithDeclaredProfileV1 {
            binding,
            bounded_member_draft,
            onboarding_profile_input,
        } => (
            binding.clone(),
            bounded_member_draft,
            onboarding_profile_input,
        ),
        other => panic!("expected one atomic create effect, got {other:?}"),
    };
    assert_eq!(bounded_member_draft.display_name(), secret_label);
    assert_eq!(bounded_member_draft.relationship(), RelationshipV1::Child);
    assert!(onboarding_profile_input.profile_data().is_ok());
    assert_eq!(
        dispatch(&mut model, Action::CancelOrExit),
        vec![Effect::CancelHouseholdOperationV1 {
            binding: binding.clone(),
        }]
    );

    let subject = HouseholdSubjectId::member(
        MemberId::parse_preserved("550e8400-e29b-41d4-a716-446655440000").unwrap(),
    );
    let apply = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdMutationCommittedV1 {
            binding: binding.clone(),
            kind: HouseholdMutationKindV1::CreateMember,
            resulting_household_revision: HouseholdRevision::new(2).unwrap(),
            affected_subject: Some(subject.clone()),
            active_scope: HouseholdScope::Subject(subject.clone()),
            bounded_active_label: secret_label.into(),
        }),
    );
    assert_eq!(model.household_chrome_label(), Some("Me"));
    assert!(matches!(
        apply.as_slice(),
        [Effect::ApplyCommittedHouseholdContextV1 { .. }]
    ));
    assert!(
        dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdContextAppliedV1 {
                binding,
                resulting_household_revision: HouseholdRevision::new(2).unwrap(),
                active_scope: HouseholdScope::Subject(subject),
                bounded_active_label: secret_label.into(),
            })
        )
        .is_empty()
    );
    assert_eq!(model.household_chrome_label(), Some(secret_label));
    assert!(
        model
            .scrollback
            .entries()
            .iter()
            .all(|entry| !entry.text.contains(old_subject_result))
    );
    let applied = &model.scrollback.entries().back().unwrap().text;
    assert!(applied.contains(&format!("For: {secret_label}")));
    assert!(applied.contains("Their food profile is saved on this device"));
    assert!(!applied.contains("declared dietary profile"));
    assert!(!applied.contains("Hosted guidance"));
}

#[test]
fn indistinguishable_duplicate_members_fail_closed_without_subject_selection() {
    let mut model = AppModel::default();
    let first = member(
        "member-one",
        "Duplicate",
        HouseholdProfileStateV1::LocalOnly,
    );
    let second = member(
        "member-two",
        "Duplicate",
        HouseholdProfileStateV1::LocalOnly,
    );
    bootstrap(
        &mut model,
        HouseholdPresentationModeV1::NativeEnabled,
        vec![owner(), first.clone(), second.clone()],
        HouseholdScope::Subject(HouseholdSubjectId::self_()),
    );
    let load = submit_text(&mut model, "/for Duplicate");
    let evidence = load_evidence(&load[0]);
    assert!(
        dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                operation_id: evidence.operation_id,
                session_mode_generation: evidence.generation,
                reducer_correlation: evidence.correlation,
                purpose: evidence.purpose,
                account_binding_digest: evidence.digest,
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![owner(), first, second.clone()],
            })
        )
        .is_empty()
    );
    let copy = &model.scrollback.entries().back().unwrap().text;
    assert!(copy.contains("can’t distinguish them safely"), "{copy}");
    assert!(copy.contains("Make their labels unique"), "{copy}");
    assert!(!copy.contains("member-one"), "{copy}");
    assert!(!copy.contains("member-two"), "{copy}");
    let selected = submit_text(&mut model, "2");
    assert!(
        selected
            .iter()
            .all(|effect| !matches!(effect, Effect::SelectHouseholdScopeV1 { .. }))
    );
}

#[test]
fn duplicate_labels_with_distinct_relationships_are_stable_subject_bound() {
    let mut model = AppModel::default();
    let first = member_with_relationship(
        "member-one",
        "Duplicate",
        RelationshipV1::Child,
        HouseholdProfileStateV1::LocalOnly,
    );
    let second = member_with_relationship(
        "member-two",
        "Duplicate",
        RelationshipV1::Friend,
        HouseholdProfileStateV1::LocalOnly,
    );
    bootstrap(
        &mut model,
        HouseholdPresentationModeV1::NativeEnabled,
        vec![owner(), first.clone(), second.clone()],
        HouseholdScope::Subject(HouseholdSubjectId::self_()),
    );
    let load = submit_text(&mut model, "/for Duplicate");
    let evidence = load_evidence(&load[0]);
    assert!(
        dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                operation_id: evidence.operation_id,
                session_mode_generation: evidence.generation,
                reducer_correlation: evidence.correlation,
                purpose: evidence.purpose,
                account_binding_digest: evidence.digest,
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![owner(), first, second.clone()],
            })
        )
        .is_empty()
    );
    let copy = &model.scrollback.entries().back().unwrap().text;
    assert!(copy.contains("1. Duplicate (child)"), "{copy}");
    assert!(copy.contains("2. Duplicate (friend)"), "{copy}");
    assert!(!copy.contains("member-one"), "{copy}");
    assert!(!copy.contains("member-two"), "{copy}");
    let selected = submit_text(&mut model, "2");
    let Effect::SelectHouseholdScopeV1 {
        binding,
        selected_scope,
    } = &selected[0]
    else {
        panic!("expected typed select effect");
    };
    assert_eq!(
        selected_scope,
        &HouseholdScope::Subject(second.subject().clone())
    );
    let apply = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdMutationCommittedV1 {
            binding: binding.clone(),
            kind: HouseholdMutationKindV1::SelectScope,
            resulting_household_revision: HouseholdRevision::new(2).unwrap(),
            affected_subject: Some(second.subject().clone()),
            active_scope: selected_scope.clone(),
            bounded_active_label: "Duplicate".into(),
        }),
    );
    assert_eq!(model.household_chrome_label(), Some("Me"));
    assert_eq!(apply.len(), 1);
}

#[test]
fn existing_member_onboarding_uses_typed_subject_and_can_cancel_before_dispatch() {
    let mut model = AppModel::default();
    let incomplete = member(
        "member-incomplete",
        "Needs profile",
        HouseholdProfileStateV1::Incomplete,
    );
    bootstrap(
        &mut model,
        HouseholdPresentationModeV1::NativeEnabled,
        vec![owner(), incomplete.clone()],
        HouseholdScope::Subject(HouseholdSubjectId::self_()),
    );
    let load = submit_text(&mut model, "/onboard --for Needs profile");
    let evidence = load_evidence(&load[0]);
    let _ = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id: evidence.operation_id,
            session_mode_generation: evidence.generation,
            reducer_correlation: evidence.correlation,
            purpose: evidence.purpose,
            account_binding_digest: evidence.digest,
            household_revision: HouseholdRevision::new(1).unwrap(),
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            members: vec![owner(), incomplete.clone()],
        }),
    );
    assert!(
        model
            .scrollback
            .entries()
            .back()
            .unwrap()
            .text
            .contains("Needs profile")
    );
    assert!(submit_text(&mut model, "none").is_empty());
    assert!(dispatch(&mut model, Action::CancelOrExit).is_empty());
    assert!(
        model
            .scrollback
            .entries()
            .back()
            .unwrap()
            .text
            .contains("No member or member profile was changed")
    );

    let load = submit_text(&mut model, "/onboard --for member-incomplete");
    let evidence = load_evidence(&load[0]);
    let _ = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id: evidence.operation_id,
            session_mode_generation: evidence.generation,
            reducer_correlation: evidence.correlation,
            purpose: evidence.purpose,
            account_binding_digest: evidence.digest,
            household_revision: HouseholdRevision::new(1).unwrap(),
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            members: vec![owner(), incomplete.clone()],
        }),
    );
    answer_complete_profile(&mut model);
    let save = submit_text(&mut model, "save");
    let Effect::SaveMemberDeclaredProfileV1 {
        subject,
        expected_profile_revision,
        ..
    } = &save[0]
    else {
        panic!("expected typed existing-member profile save");
    };
    assert_eq!(subject, incomplete.subject());
    assert_eq!(*expected_profile_revision, None);
}

#[test]
fn context_apply_failure_clears_chrome_and_forces_bound_bootstrap() {
    let mut model = AppModel::default();
    let selected = member(
        "member-selected",
        "Selected",
        HouseholdProfileStateV1::LocalOnly,
    );
    bootstrap(
        &mut model,
        HouseholdPresentationModeV1::NativeEnabled,
        vec![owner(), selected.clone()],
        HouseholdScope::Subject(HouseholdSubjectId::self_()),
    );
    let load = submit_text(&mut model, "/for member-selected");
    let evidence = load_evidence(&load[0]);
    let select_effects = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id: evidence.operation_id,
            session_mode_generation: evidence.generation,
            reducer_correlation: evidence.correlation,
            purpose: evidence.purpose,
            account_binding_digest: evidence.digest,
            household_revision: HouseholdRevision::new(1).unwrap(),
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            members: vec![owner(), selected.clone()],
        }),
    );
    let Effect::SelectHouseholdScopeV1 {
        binding,
        selected_scope,
    } = &select_effects[0]
    else {
        panic!("expected select effect after the fresh management load")
    };
    let binding = binding.clone();
    let selected_scope = selected_scope.clone();
    let _ = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdMutationCommittedV1 {
            binding: binding.clone(),
            kind: HouseholdMutationKindV1::SelectScope,
            resulting_household_revision: HouseholdRevision::new(2).unwrap(),
            affected_subject: Some(selected.subject().clone()),
            active_scope: selected_scope,
            bounded_active_label: "Selected".into(),
        }),
    );
    let bootstrap = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdContextApplyFailedV1 {
            binding,
            resulting_household_revision: HouseholdRevision::new(2).unwrap(),
            reason: heyfood_tui::HouseholdContextApplyFailureV1::StateChanged,
        }),
    );
    assert_eq!(bootstrap.len(), 1);
    assert_eq!(
        load_evidence(&bootstrap[0]).purpose,
        HouseholdManagementLoadPurposeV1::Bootstrap
    );
    assert_eq!(model.household_chrome_label(), None);
}

#[test]
fn rollback_panel_is_read_only_and_chrome_is_bounded() {
    let mut model = AppModel::default();
    bootstrap(
        &mut model,
        HouseholdPresentationModeV1::NativeRollbackReadOnly,
        vec![owner()],
        HouseholdScope::Subject(HouseholdSubjectId::self_()),
    );
    assert!(submit_text(&mut model, "/household add").is_empty());
    assert!(submit_text(&mut model, "/for me").is_empty());
    let panel = submit_text(&mut model, "/household");
    let evidence = load_evidence(&panel[0]);
    let _ = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id: evidence.operation_id,
            session_mode_generation: evidence.generation,
            reducer_correlation: evidence.correlation,
            purpose: evidence.purpose,
            account_binding_digest: evidence.digest,
            household_revision: HouseholdRevision::new(1).unwrap(),
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            members: vec![owner()],
        }),
    );
    assert!(
        !model
            .scrollback
            .entries()
            .back()
            .unwrap()
            .text
            .contains("Add a household member:")
    );
    for width in [40, 80, 120] {
        let chrome = household_chrome_copy(&model, width).unwrap();
        assert_eq!(chrome, "For: Me");
        assert!(!chrome.chars().any(char::is_control));
    }
}

#[test]
fn restarted_member_and_everyone_scopes_are_hosted_ready() {
    let roster = || {
        vec![
            owner(),
            member(
                "550e8400-e29b-41d4-a716-446655440000",
                "Maya",
                HouseholdProfileStateV1::LocalOnly,
            ),
        ]
    };
    for scope in [
        HouseholdScope::Subject(HouseholdSubjectId::member(
            MemberId::parse_preserved("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        )),
        HouseholdScope::Everyone,
    ] {
        let mut model = AppModel::default();
        bootstrap(
            &mut model,
            HouseholdPresentationModeV1::NativeEnabled,
            roster(),
            scope,
        );
        assert!(model.household_management_ready());

        assert!(matches!(
            submit_text(&mut model, "What should we eat?").as_slice(),
            [Effect::SubmitTurn { prompt, .. }] if prompt == "What should we eat?"
        ));

        let mut management = AppModel::default();
        bootstrap(
            &mut management,
            HouseholdPresentationModeV1::NativeEnabled,
            roster(),
            HouseholdScope::Everyone,
        );
        let panel = submit_text(&mut management, "/household");
        assert!(matches!(
            panel.as_slice(),
            [Effect::LoadHouseholdManagementV1 {
                purpose: HouseholdManagementLoadPurposeV1::Panel,
                ..
            }]
        ));

        let mut switching = AppModel::default();
        bootstrap(
            &mut switching,
            HouseholdPresentationModeV1::NativeEnabled,
            roster(),
            HouseholdScope::Everyone,
        );
        let select = submit_text(&mut switching, "/for me");
        assert!(matches!(
            select.as_slice(),
            [Effect::LoadHouseholdManagementV1 {
                purpose: HouseholdManagementLoadPurposeV1::SelectScope,
                ..
            }]
        ));
    }
}

#[test]
fn household_sensitive_debug_carriers_redact_unique_canaries() {
    let canary = "D3-DEBUG-CANARY-X9";
    let entry = SemanticEntry {
        speaker: Speaker::User,
        text: canary.into(),
        streaming: false,
    };
    assert!(!format!("{entry:?}").contains(canary));

    let draft = heyfood_tui::BoundedHouseholdMemberDraftV1::new(
        canary,
        RelationshipV1::Friend,
        heyfood_tui::HouseholdAgeEvidenceInputV1::Unknown,
    )
    .unwrap();
    assert!(!format!("{draft:?}").contains(canary));
    let event = RuntimeEvent::HouseholdMutationCommittedV1 {
        binding: heyfood_tui::HouseholdOperationBindingV1::new(
            heyfood_tui::HouseholdOperationIdV1::new(1).unwrap(),
            HouseholdModeGenerationV1::new(1).unwrap(),
            HouseholdAccountBindingDigestV1::from_bytes([9; 32]),
            HouseholdRevision::new(1).unwrap(),
            heyfood_tui::HouseholdReducerCorrelationV1::new(1).unwrap(),
        ),
        kind: HouseholdMutationKindV1::CreateMember,
        resulting_household_revision: HouseholdRevision::new(2).unwrap(),
        affected_subject: Some(HouseholdSubjectId::member(
            MemberId::parse_preserved(canary).unwrap(),
        )),
        active_scope: HouseholdScope::Everyone,
        bounded_active_label: canary.into(),
    };
    assert!(!format!("{event:?}").contains(canary));
    let action = Action::InsertText(canary.into());
    assert!(!format!("{action:?}").contains(canary));
}
