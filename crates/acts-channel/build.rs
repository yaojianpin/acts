fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "codegen")]
    {
        // SAFETY: build scripts run single-threaded in their own process before
        // any threads spawn, so mutating the env cannot race.
        unsafe {
            std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
        }
        tonic_build::configure()
            .out_dir("proto")
            .compile_protos(&["acts.proto"], &["proto"])?;
    }
    Ok(())
}
