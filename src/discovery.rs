//! Pinned core capabilities; optional UI is deliberately not negotiated.
use rmcp::model::*;

pub(crate) fn frontend_capabilities(upstream: &ServerCapabilities) -> ServerCapabilities {
    let mut caps = ServerCapabilities::builder().enable_tools().build();
    if upstream.resources.is_some() {
        caps.resources = Some(Default::default());
    }
    if upstream.prompts.is_some() {
        caps.prompts = Some(Default::default());
    }
    // The frontend catalog is immutable. Upstream listChanged invalidates the
    // pin, not a client's view. Subscriptions, completion and UI stay disabled.
    caps
}

pub(crate) fn first_page(request: Option<PaginatedRequestParams>) -> Result<(), ErrorData> {
    if request.and_then(|p| p.cursor).is_some() {
        return Err(ErrorData::invalid_params("No further catalog pages", None));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advertise_only_implemented_immutable_core_catalogs() {
        let upstream = serde_json::from_value(serde_json::json!({
            "tools":{"listChanged":true},"prompts":{"listChanged":true},
            "resources":{"listChanged":true,"subscribe":false},
            "extensions":{"io.modelcontextprotocol/ui":{}},"experimental":{}
        }))
        .unwrap();
        let caps = frontend_capabilities(&upstream);
        assert!(caps.tools.is_some() && caps.resources.is_some() && caps.prompts.is_some());
        assert!(caps.extensions.is_none() && caps.experimental.is_none());
        assert_ne!(caps.resources.unwrap().list_changed, Some(true));
        assert_ne!(caps.prompts.unwrap().list_changed, Some(true));
        assert_ne!(caps.tools.unwrap().list_changed, Some(true));
    }
}
