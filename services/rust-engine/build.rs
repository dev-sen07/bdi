fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compila bdi.proto. `tonic-build` generará bdi.rs en OUT_DIR.
    // Usamos el proto compartido en la raíz del proyecto.
    tonic_build::compile_protos("../../proto/bdi.proto")?;
    Ok(())
}
