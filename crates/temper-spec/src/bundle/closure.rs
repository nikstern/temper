use super::*;

/// Compute the immutable generated-client closure for scoped CSDL and IOA inputs.
pub fn scoped_module_data_closure_digest(
    csdl_xml: &str,
    ioa_sources: Vec<IoaSourceInput>,
) -> Result<String, BundleError> {
    scoped_module_data_closure_digest_with_version(
        csdl_xml,
        ioa_sources,
        SCOPED_SPEC_BUNDLE_CONTRACT_V2,
    )
}

/// Compute a generated-client closure under an explicit bundle contract.
pub fn scoped_module_data_closure_digest_with_version(
    csdl_xml: &str,
    ioa_sources: Vec<IoaSourceInput>,
    canonicalization_version: &str,
) -> Result<String, BundleError> {
    if csdl_xml.len() > MAX_CSDL_BYTES {
        return Err(BundleError::new(
            BundleErrorCode::BudgetExceeded,
            format!("CSDL exceeds v1 byte budget {MAX_CSDL_BYTES}"),
        ));
    }
    let ioa_specs = canonical_ioa_specs(ioa_sources)?;
    let canonical_csdl = match canonicalization_version {
        SCOPED_SPEC_BUNDLE_CONTRACT_V1 => canonical_csdl(csdl_xml)?,
        SCOPED_SPEC_BUNDLE_CONTRACT_V2 => {
            let automata = parsed_qualified_automata(&ioa_specs)?;
            CanonicalSpecModel::link_v2(csdl_xml, &automata)?
                .emitted_csdl_xml()
                .to_string()
        }
        _ => {
            return Err(BundleError::new(
                BundleErrorCode::InvalidBundle,
                format!("unsupported canonicalization version '{canonicalization_version}'"),
            ));
        }
    };
    validate_bundle_contracts(&canonical_csdl, &ioa_specs, canonicalization_version)?;
    Ok(module_data_closure_digest(
        canonicalization_version,
        &canonical_csdl,
        &ioa_specs,
    ))
}
