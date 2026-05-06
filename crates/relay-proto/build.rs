fn main() {
    let proto_file = "proto/relay/v1/relay.proto";
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("failed to locate vendored protoc");
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile(&[proto_file], &["proto"])
        .expect("failed to compile protos");
}
