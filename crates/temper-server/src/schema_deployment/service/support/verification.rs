use super::*;

pub(super) fn canonical_automata(
    record: &SchemaDeploymentRecord,
) -> Result<Vec<temper_spec::Automaton>, ServiceError> {
    match record.bundle.canonicalization_version.as_str() {
        temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V2 => {
            let csdl = temper_spec::parse_csdl(&record.bundle.canonical_csdl).map_err(|error| {
                ServiceError::new("verification_failed", error.to_string(), false)
            })?;
            let sources = record
                .bundle
                .canonical_ioa
                .iter()
                .map(|(entity_type, source)| temper_spec::IoaSourceInput {
                    entity_type: entity_type.clone(),
                    source: source.clone(),
                })
                .collect::<Vec<_>>();
            temper_spec::CanonicalSpecModel::link_v2_sources(&csdl, &sources)
                .map_err(|error| ServiceError::new("verification_failed", error.to_string(), false))
                .map(|model| {
                    model
                        .entities()
                        .values()
                        .filter_map(|entity| entity.automaton().cloned())
                        .collect()
                })
        }
        temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V1 => record
            .bundle
            .canonical_ioa
            .values()
            .map(|source| {
                temper_spec::parse_automaton(source).map_err(|error| {
                    ServiceError::new("verification_failed", error.to_string(), false)
                })
            })
            .collect(),
        version => Err(ServiceError::new(
            "verification_failed",
            format!("unsupported canonicalization version '{version}'"),
            false,
        )),
    }
}
