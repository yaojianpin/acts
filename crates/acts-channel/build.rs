fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "codegen")]
    {
        // SAFETY: build scripts run single-threaded in their own process before
        // any threads spawn, so mutating the env cannot race.
        unsafe {
            std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
        }
        // `tonic-build` 0.14 generates only the transport skeleton; the prost
        // codec the bindings use lives in `tonic-prost`, so the codegen half is
        // `tonic-prost-build`.
        tonic_prost_build::configure()
            .out_dir("proto")
            .compile_protos(&["acts.proto"], &["proto"])?;
    }
    Ok(())
}
