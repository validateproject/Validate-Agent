fn main() {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile(&["proto/executor.proto"], &["proto"])
        .expect("failed to compile executor proto");
}
