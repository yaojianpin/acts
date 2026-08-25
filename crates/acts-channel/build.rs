fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "codegen")]
    tonic_build::configure()
        .out_dir("proto")
        .compile_protos(&["acts.proto"], &["proto"])?;
    Ok(())
}
