// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

//! Messages the language server sends to its editor client.

use i_slint_editor_preview::VersionedDiagnostics;
use i_slint_live_preview::protocol::SourceFileVersion;

pub fn notify_lsp_diagnostics(
    sender: &crate::ServerNotifier,
    uri: lsp_types::Url,
    version: SourceFileVersion,
    diagnostics: Vec<lsp_types::Diagnostic>,
) -> Option<()> {
    sender
        .send_notification::<lsp_types::notification::PublishDiagnostics>(
            lsp_types::PublishDiagnosticsParams { uri, diagnostics, version },
        )
        .ok()
}

#[cfg(any(feature = "preview-external", feature = "preview-engine"))]
fn show_document_request_from_element_callback(
    uri: lsp_types::Url,
    range: lsp_types::Range,
    take_focus: bool,
) -> Option<lsp_types::ShowDocumentParams> {
    if range.start.character == 0 || range.end.character == 0 {
        return None;
    }

    Some(lsp_types::ShowDocumentParams {
        uri,
        external: Some(false),
        take_focus: Some(take_focus),
        selection: Some(range),
    })
}

#[cfg(any(feature = "preview-external", feature = "preview-engine"))]
pub async fn send_show_document_to_editor(
    sender: crate::ServerNotifier,
    file: lsp_types::Url,
    range: lsp_types::Range,
    take_focus: bool,
) {
    let Some(params) = show_document_request_from_element_callback(file, range, take_focus) else {
        return;
    };
    let Ok(fut) = sender.send_request::<lsp_types::request::ShowDocument>(params) else {
        return;
    };

    let _ = fut.await;
}

/// Publish the diagnostics returned by the document lifecycle functions to the LSP client.
pub fn publish_diagnostics(
    server_notifier: &crate::ServerNotifier,
    diagnostics: VersionedDiagnostics,
) {
    for (uri, version, diagnostics) in diagnostics {
        let _ = notify_lsp_diagnostics(server_notifier, uri, version, diagnostics);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    #[test]
    fn standard_diagnostics_are_published_without_preview() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let notifier = crate::ServerNotifier::new(sender, Default::default());
        let uri = lsp_types::Url::parse("file:///test.slint").unwrap();
        let diagnostic = lsp_types::Diagnostic::new_simple(Default::default(), "error".into());
        super::publish_diagnostics(
            &notifier,
            vec![
                (uri.clone(), Some(1), vec![diagnostic.clone()]),
                (uri.clone(), Some(2), Vec::new()),
            ],
        );
        for (version, diagnostics) in [(1, vec![diagnostic]), (2, Vec::new())] {
            let lsp_server::Message::Notification(notification) =
                receiver.try_recv().expect("diagnostics were not published")
            else {
                panic!("expected diagnostic notification")
            };
            assert_eq!(notification.method, "textDocument/publishDiagnostics");
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            assert_eq!(params.uri, uri);
            assert_eq!(params.version, Some(version));
            assert_eq!(params.diagnostics, diagnostics);
        }
    }
}
