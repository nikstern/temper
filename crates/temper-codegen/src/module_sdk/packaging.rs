use temper_wasm_sdk::data::{ArtifactModuleSdkBinding, bind_module_sdk_artifact};

use super::{GeneratedModuleSdk, ModuleSdkCodegenError, PackagedModuleSdk, hex_sha256};

/// Bind generated metadata into compiled WebAssembly and finalize its digest.
///
/// Build tooling calls this after compiling [`GeneratedModuleSdk::source`] and
/// before constructing the published application bundle.
pub fn package_generated_module_sdk(
    wasm: &[u8],
    mut generated: GeneratedModuleSdk,
) -> Result<PackagedModuleSdk, ModuleSdkCodegenError> {
    let binding = ArtifactModuleSdkBinding::from_manifest(&generated.manifest)
        .map_err(ModuleSdkCodegenError::ArtifactBinding)?;
    let wasm =
        bind_module_sdk_artifact(wasm, &binding).map_err(ModuleSdkCodegenError::ArtifactBinding)?;
    generated.manifest.artifact_digest = hex_sha256(&wasm);
    Ok(PackagedModuleSdk {
        wasm,
        manifest: generated.manifest,
    })
}
