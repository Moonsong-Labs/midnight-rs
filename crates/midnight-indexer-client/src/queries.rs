/// Fetch a block by optional offset (hash or height). Returns latest if no offset.
pub const BLOCK_QUERY: &str = r#"
    query BlockQuery($offset: BlockOffset) {
        block(offset: $offset) {
            hash
            height
            protocolVersion
            timestamp
            author
            ledgerParameters
        }
    }
"#;
