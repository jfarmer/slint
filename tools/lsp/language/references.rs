// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use crate::editor_preview::{DocumentCache, token_info};
use i_slint_compiler::parser::{SyntaxKind, SyntaxToken};
use lsp_types::Location;

pub fn references(
    document_cache: &DocumentCache,
    token: SyntaxToken,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    if token.kind() != SyntaxKind::Identifier {
        return None;
    }
    let symbol = token_info::token_info(document_cache, token)?;
    let mut locations = Vec::new();
    for (uri, document) in document_cache.all_url_documents() {
        if uri.scheme() == "builtin" {
            continue;
        }
        for occurrence in token_info::symbol_occurrences(document_cache, &symbol, document) {
            if !include_declaration && is_declaration(&occurrence) {
                continue;
            }
            locations.push(Location::new(
                uri.clone(),
                crate::util::token_to_lsp_range(&occurrence, document_cache.format),
            ));
        }
    }
    locations.sort_by(|a, b| a.uri.cmp(&b.uri).then(a.range.start.cmp(&b.range.start)));
    locations.dedup();
    Some(locations)
}

fn is_declaration(token: &SyntaxToken) -> bool {
    let node = token.parent();
    match node.kind() {
        SyntaxKind::DeclaredIdentifier => node.parent().is_some_and(|parent| {
            matches!(
                parent.kind(),
                SyntaxKind::Component
                    | SyntaxKind::PropertyDeclaration
                    | SyntaxKind::CallbackDeclaration
                    | SyntaxKind::Function
                    | SyntaxKind::StructDeclaration
                    | SyntaxKind::EnumDeclaration
                    | SyntaxKind::RepeatedElement
            )
        }),
        SyntaxKind::SubElement
        | SyntaxKind::InternalName
        | SyntaxKind::ExportName
        | SyntaxKind::ObjectTypeMember
        | SyntaxKind::EnumValue => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::test;
    use i_slint_compiler::parser::TextSize;
    use lsp_types::Url;

    fn location(dc: &DocumentCache, uri: &Url, source: &str, anchor: &str) -> Location {
        let offset = TextSize::new(source.find(anchor).unwrap() as u32);
        let doc = dc.get_document(uri).unwrap().node.as_ref().unwrap();
        let token = crate::language::token_at_offset(doc, offset).unwrap();
        assert_eq!(token.text(), anchor.split("/*").next().unwrap());
        Location::new(uri.clone(), crate::util::token_to_lsp_range(&token, dc.format))
    }

    fn check(
        dc: &DocumentCache,
        uri: &Url,
        source: &str,
        query: &str,
        expected: &[&str],
        include_declaration: bool,
    ) {
        let offset = TextSize::new(source.find(query).unwrap() as u32);
        let doc = dc.get_document(uri).unwrap().node.as_ref().unwrap();
        let token = crate::language::token_at_offset(doc, offset).unwrap();
        let actual = references(dc, token, include_declaration)
            .unwrap_or_else(|| panic!("unresolved query: {query}"));
        let mut expected: Vec<_> = expected.iter().map(|a| location(dc, uri, source, a)).collect();
        expected.sort_by_key(|l| l.range.start);
        assert_eq!(actual, expected, "query: {query}");
    }

    #[test]
    fn members_in_scopes_and_inheritance() {
        let source = r#"
component Base {
    in-out property <int> the-prop/*decl*/;
    callback fired/*callback*/;
    public function run/*function*/() { fired/*call1*/(); }
}
component Derived inherits Base {
    the_prop/*binding*/: root.the-prop/*read*/;
    changed the-prop/*changed*/ => { root.run/*call2*/(); }
    fired/*connection*/ => { root.run/*call3*/(); }
    child-id/*id*/ := Rectangle {
        in-out property <int> the-prop/*other*/;
        callback fired;
        function run() { fired(); }
        x: self.the-prop/*other-read*/;
        y: child_id/*id-read*/.the_prop/*other-qualified*/;
    }
    states [ active: { child_id/*state-id*/.the_prop/*state-prop*/: 3; } ]
    animate child-id/*animation-id*/.the-prop/*animation-prop*/ { duration: 1ms; }
}
component Unrelated {
    in-out property <int> the-prop;
    callback fired;
    function run() { fired(); }
    x: the-prop;
}"#;
        let (mut dc, uri, _) = test::loaded_document_cache(source.into());
        for format in
            [crate::editor_preview::ByteFormat::Utf8, crate::editor_preview::ByteFormat::Utf16]
        {
            dc.format = format;
            let uses = ["the_prop/*binding*/", "the-prop/*read*/", "the-prop/*changed*/"];
            for query in ["the-prop/*decl*/", "the-prop/*read*/", "the_prop/*binding*/"] {
                check(&dc, &uri, source, query, &uses, false);
                let mut all = vec!["the-prop/*decl*/"];
                all.extend(uses);
                check(&dc, &uri, source, query, &all, true);
            }
            check(
                &dc,
                &uri,
                source,
                "fired/*callback*/",
                &["fired/*call1*/", "fired/*connection*/"],
                false,
            );
            check(
                &dc,
                &uri,
                source,
                "run/*call2*/",
                &["run/*function*/", "run/*call2*/", "run/*call3*/"],
                true,
            );
            check(
                &dc,
                &uri,
                source,
                "child-id/*id*/",
                &[
                    "child-id/*id*/",
                    "child_id/*id-read*/",
                    "child_id/*state-id*/",
                    "child-id/*animation-id*/",
                ],
                true,
            );
            check(
                &dc,
                &uri,
                source,
                "child_id/*id-read*/",
                &["child_id/*id-read*/", "child_id/*state-id*/", "child-id/*animation-id*/"],
                false,
            );
            check(
                &dc,
                &uri,
                source,
                "the-prop/*other*/",
                &[
                    "the-prop/*other*/",
                    "the-prop/*other-read*/",
                    "the_prop/*other-qualified*/",
                    "the_prop/*state-prop*/",
                    "the-prop/*animation-prop*/",
                ],
                true,
            );
        }
    }

    #[test]
    fn shadowed_members() {
        let source = r#"
component Base {
    @shadowable in-out property <int> prop/*base*/;
    x: self.prop/*base-use*/;
}
component Derived inherits Base {
    in-out property <int> prop/*derived*/;
    y: self.prop/*derived-use*/;
}
component App { Derived {} }
"#;
        let (dc, uri, _) = test::loaded_document_cache_with_experimental(source.into());
        check(&dc, &uri, source, "prop/*base*/", &["prop/*base*/", "prop/*base-use*/"], true);
        check(
            &dc,
            &uri,
            source,
            "prop/*derived-use*/",
            &["prop/*derived*/", "prop/*derived-use*/"],
            true,
        );
    }

    #[test]
    fn fields_variants_and_position_encoding() {
        let source = r#"
struct Data { field/*field*/: int }
struct Other { field: int }
enum Direction { left/*variant*/, right }
component App {
    in property <Data> data;
    in property <Other> other;
    in property <Direction> direction: Direction.left/*qualified*/;
    /* 😀 */ x: data.field/*access*/;
    y: other.field;
    out property <bool> yes: direction == left/*bare*/;
}"#;
        let (mut dc, uri, _) = test::loaded_document_cache(source.into());
        for format in
            [crate::editor_preview::ByteFormat::Utf8, crate::editor_preview::ByteFormat::Utf16]
        {
            dc.format = format;
            check(
                &dc,
                &uri,
                source,
                "field/*field*/",
                &["field/*field*/", "field/*access*/"],
                true,
            );
            check(
                &dc,
                &uri,
                source,
                "left/*qualified*/",
                &["left/*variant*/", "left/*qualified*/", "left/*bare*/"],
                true,
            );
            check(&dc, &uri, source, "field/*access*/", &["field/*access*/"], false);
            assert_eq!(
                location(&dc, &uri, source, "field/*access*/").range.start.character,
                if format == crate::editor_preview::ByteFormat::Utf8 { 23 } else { 21 }
            );
        }
    }

    #[test]
    fn import_export_aliases() {
        let source = "export component Original/*decl*/ {}";
        let (dc, uri, _) = test::loaded_document_cache(source.into());
        let mut ctx = test::mock_context_with_document_cache(dc);
        let middle = uri.join("middle.slint").unwrap();
        let middle_source = format!(
            "export {{ Original as Public }} from \"{}\";",
            uri.to_file_path().unwrap().display()
        );
        spin_on::spin_on(ctx.session.load_document_impl(
            middle_source.clone(),
            middle.clone(),
            Some(1),
        ));
        let app = uri.join("app.slint").unwrap();
        let app_source =
            "import { Public as Local } from \"middle.slint\"; component App { Local {} }";
        spin_on::spin_on(ctx.session.load_document_impl(app_source.into(), app.clone(), Some(1)));
        let dc = &ctx.session.document_cache;
        let doc = dc.get_document(&uri).unwrap().node.as_ref().unwrap();
        let token = crate::language::token_at_offset(
            doc,
            TextSize::new(source.find("Original").unwrap() as u32),
        )
        .unwrap();
        let actual = references(dc, token, true).unwrap();
        let texts: Vec<_> = actual
            .iter()
            .map(|l| {
                let (doc, start) = dc.get_document_and_offset(&l.uri, &l.range.start).unwrap();
                let (_, end) = dc.get_document_and_offset(&l.uri, &l.range.end).unwrap();
                let text = doc.node.as_ref().unwrap().source_file.source().unwrap();
                (l.uri.clone(), text[usize::from(start)..usize::from(end)].to_string())
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                (app.clone(), "Public".into()),
                (app.clone(), "Local".into()),
                (app, "Local".into()),
                (uri, "Original".into()),
                (middle.clone(), "Original".into()),
                (middle, "Public".into())
            ]
        );
    }

    #[test]
    fn standard_request_is_read_only() {
        let source = "export component App { in-out property <int> value; x: value; }";
        let (dc, uri, _) = test::loaded_document_cache(source.into());
        let declaration = source.find("value;").unwrap();
        let doc = dc.get_document(&uri).unwrap().node.as_ref().unwrap();
        let position = crate::util::text_size_to_lsp_position(
            &doc.source_file,
            TextSize::new(declaration as u32),
            dc.format,
        );
        let mut ctx = test::mock_context_with_document_cache(dc);
        let mut handlers = crate::language::RequestHandler::default();
        crate::language::register_request_handlers(&mut handlers);
        assert_eq!(
            crate::language::server_initialize_result(&Default::default())
                .capabilities
                .references_provider,
            Some(lsp_types::OneOf::Left(true))
        );
        let revision = ctx.session.document_cache.revision();
        let response = handlers.0["textDocument/references"](
            serde_json::json!({
                "textDocument": { "uri": uri }, "position": position,
                "context": { "includeDeclaration": false }
            }),
            &mut ctx,
        )
        .unwrap();
        let locations: Vec<Location> = serde_json::from_value(response).unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(ctx.session.document_cache.revision(), revision);
        assert_eq!(
            ctx.session
                .document_cache
                .get_document(&uri)
                .unwrap()
                .node
                .as_ref()
                .unwrap()
                .text()
                .to_string(),
            source
        );
        assert!(!ctx.host_language_rename_dont_ask_again.get());
    }
}
