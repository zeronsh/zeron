use zeron_proto::{AccessMode, SandboxLevel, cap_access};
#[test]
fn security_review_auto_ceiling_survives_unsupported_agent() {
    let (_, capped, _) = cap_access(
        (SandboxLevel::WorkspaceWrite, AccessMode::FullAccess),
        (SandboxLevel::WorkspaceWrite, AccessMode::Auto),
    );
    let effective = capped.honored_by(&[AccessMode::FullAccess]);
    println!("Requested: FullAccess; parent ceiling: Auto; capped: {capped:?}; effective ACP access: {effective:?}");
    assert_ne!(effective, AccessMode::FullAccess, "Auto must not silently become FullAccess");
}
