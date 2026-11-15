    #[test]
    fn security_review_mcp_error_is_not_a_reviewer_verdict() {
        let part = folded_tool(
            "mcp__example__write",
            json!({"path": "/tmp/audit-marker"}),
            json!("Permission to use write has been denied by downstream service after partial write."),
            true,
        );
        let zeron_doc::MessagePart::Tool { review, .. } = part else { panic!("not a tool") };
        println!("Ordinary MCP error produced reviewer metadata: {review:?}");
        assert!(review.is_none(), "Tool output is not trusted reviewer provenance");
    }

