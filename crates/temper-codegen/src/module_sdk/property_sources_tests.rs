use std::collections::BTreeSet;

use temper_spec::bundle::IoaSourceInput;
use temper_spec::csdl::parse_csdl;
use temper_wasm_sdk::data::{
    DataOperationKind, EntityDataGrant, ManifestValueSourceV1, ModuleDataGrant,
};

use super::{ModuleSdkCodegenError, generate_module_sdk};

const IOA: &str = r#"[automaton]
name = "Session"
states = ["Unconfigured", "Active"]
initial = "Unconfigured"
lifecycle_property = "State"

[[action]]
name = "Activate"
kind = "input"
from = ["Unconfigured"]
to = "Active"
"#;

fn generate(properties: &str) -> Result<super::GeneratedModuleSdk, ModuleSdkCodegenError> {
    let csdl = parse_csdl(&format!(
        r#"<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Temper.Test" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EnumType Name="SessionLifecycle"><Member Name="Unconfigured"/><Member Name="Active"/></EnumType><EntityType Name="Session"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/>{properties}</EntityType><Action Name="Activate" IsBound="true"><Parameter Name="binding" Type="Temper.Test.Session" Nullable="false"/></Action><EntityContainer Name="Container"><EntitySet Name="Sessions" EntityType="Temper.Test.Session"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#
    ))
    .expect("fixture CSDL parses");
    let sources = [IoaSourceInput {
        entity_type: "Temper.Test.Session".into(),
        source: IOA.into(),
    }];
    let model =
        temper_spec::CanonicalSpecModel::link_v2_sources(&csdl, &sources).map_err(|error| {
            ModuleSdkCodegenError::InvalidIoaSource {
                entity_type: "Temper.Test.Session".into(),
                message: error.to_string(),
            }
        })?;
    generate_module_sdk(
        &model,
        "worker",
        "closure",
        "closure",
        "artifact",
        ModuleDataGrant {
            operations: BTreeSet::from([DataOperationKind::EntityGet]),
            entities: vec![EntityDataGrant {
                entity_type: "Temper.Test.Session".into(),
                ..EntityDataGrant::default()
            }],
            ..ModuleDataGrant::default()
        },
    )
}

#[test]
fn lifecycle_and_ordinary_state_sources_are_distinct() {
    let generated = generate(
        r#"<Property Name="State" Type="Edm.String" Nullable="false" DefaultValue="Unconfigured"/><Property Name="RegionState" Type="Edm.String" Nullable="false" DefaultValue="CA"/>"#,
    )
    .expect("one structural lifecycle property should bind");
    let properties = &generated.manifest.entities[0].properties;
    let source = |name: &str| {
        properties
            .iter()
            .find(|property| property.canonical_name == name)
            .map(|property| property.source)
    };
    assert_eq!(source("Id"), Some(ManifestValueSourceV1::EntityId));
    assert_eq!(
        source("State"),
        Some(ManifestValueSourceV1::LifecycleStatus)
    );
    assert_eq!(
        source("RegionState"),
        Some(ManifestValueSourceV1::StoredField)
    );
}

#[test]
fn explicit_lifecycle_enum_does_not_require_an_authored_default() {
    let generated = generate(
        r#"<Property Name="State" Type="Temper.Test.SessionLifecycle" Nullable="false"/>"#,
    )
    .expect("an exact lifecycle enum should bind unambiguously");
    assert_eq!(
        generated.manifest.entities[0].properties[1].source,
        ManifestValueSourceV1::LifecycleStatus
    );
}

#[test]
fn explicit_lifecycle_property_ignores_structurally_similar_properties() {
    let generated = generate(
        r#"<Property Name="State" Type="Edm.String" Nullable="false" DefaultValue="Unconfigured"/><Property Name="Status" Type="Edm.String" Nullable="false" DefaultValue="Unconfigured"/>"#,
    )
    .expect("explicit IOA property removes structural ambiguity");
    assert_eq!(
        generated.manifest.entities[0]
            .properties
            .iter()
            .find(|property| property.canonical_name == "State")
            .unwrap()
            .source,
        ManifestValueSourceV1::LifecycleStatus
    );
}

#[test]
fn missing_lifecycle_candidate_fails_before_binding() {
    let error = generate(
        r#"<Property Name="State" Type="Edm.String" Nullable="false" DefaultValue="CA"/>"#,
    )
    .expect_err("ordinary State property must not be treated as lifecycle");
    assert!(
        error
            .to_string()
            .contains("default 'CA' contradicts IOA initial state")
    );
}

#[test]
fn lifecycle_enum_default_must_match_ioa_initial_state() {
    let error = generate(
        r#"<Property Name="State" Type="Temper.Test.SessionLifecycle" Nullable="false" DefaultValue="Active"/>"#,
    )
    .expect_err("a contradictory lifecycle default must fail closed");
    assert!(
        error
            .to_string()
            .contains("default 'Active' contradicts IOA initial state")
    );
}

#[test]
fn mixed_bundle_generates_data_only_entity_without_lifecycle_source() {
    let xml = r#"<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Temper.Test" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="Session"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="State" Type="Edm.String" Nullable="false"/></EntityType><EntityType Name="Receipt"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="Note" Type="Edm.String" Nullable="false"/></EntityType><Action Name="Activate" IsBound="true"><Parameter Name="binding" Type="Temper.Test.Session" Nullable="false"/></Action><EntityContainer Name="Container"><EntitySet Name="Sessions" EntityType="Temper.Test.Session"/><EntitySet Name="Receipts" EntityType="Temper.Test.Receipt"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#;
    let csdl = parse_csdl(xml).expect("mixed fixture CSDL parses");
    let sources = [IoaSourceInput {
        entity_type: "Temper.Test.Session".into(),
        source: IOA.into(),
    }];
    let model = temper_spec::CanonicalSpecModel::link_v2_sources(&csdl, &sources)
        .expect("mixed model links");
    let generated = generate_module_sdk(
        &model,
        "worker",
        "closure",
        "closure",
        "artifact",
        ModuleDataGrant {
            operations: BTreeSet::from([DataOperationKind::EntityGet]),
            entities: vec![EntityDataGrant {
                entity_type: "Temper.Test.Receipt".into(),
                ..EntityDataGrant::default()
            }],
            ..ModuleDataGrant::default()
        },
    )
    .expect("data-only entity remains available to SDK generation");

    let receipt = &generated.manifest.entities[0];
    assert_eq!(receipt.entity_type, "Temper.Test.Receipt");
    assert!(receipt.lifecycle_states.is_empty());
    assert_eq!(
        receipt.properties[0].source,
        ManifestValueSourceV1::EntityId
    );
    assert_eq!(
        receipt.properties[1].source,
        ManifestValueSourceV1::StoredField
    );
}
