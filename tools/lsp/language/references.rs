// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use crate::editor_preview::{DocumentCache, token_info};
use i_slint_compiler::parser::{SyntaxKind, SyntaxToken};
use lsp_types::Location;

#[cfg(not(target_arch = "wasm32"))]
pub async fn load_workspace(ctx: &mut super::Context) {
    use i_slint_compiler::diagnostics::BuildDiagnostics;
    use std::collections::{HashMap, HashSet};

    let roots: Vec<_> = crate::host_language_search::resolve_workspace_folders(&ctx.init_param)
        .into_iter()
        .filter_map(|folder| folder.uri.to_file_path().ok())
        .collect();
    if roots.is_empty() {
        return;
    }
    let dc = &mut ctx.session.document_cache;
    let cached: HashMap<_, _> = dc
        .all_url_documents()
        .filter(|(url, _)| url.scheme() == "file")
        .map(|(url, node)| (url, node.source_file.source().unwrap_or_default().to_string()))
        .collect();
    let open: HashSet<_> = ctx
        .session
        .open_urls
        .iter()
        .filter_map(|url| i_slint_compiler::source_path::SourcePath::from_url(url).to_url())
        .collect();
    let known_paths: Vec<_> = cached
        .keys()
        .filter(|url| url.path().ends_with(".slint"))
        .filter_map(|url| url.to_file_path().ok())
        .collect();
    let sources = tokio::task::spawn_blocking(move || {
        let mut files: HashSet<_> = known_paths.into_iter().collect();
        let mut directories = roots;
        let mut visited = HashSet::new();
        while let Some(directory) = directories.pop() {
            if !visited.insert(directory.clone()) {
                continue;
            }
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::debug!("Cannot scan {}: {error}", directory.display());
                    continue;
                }
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                let path = entry.path();
                if kind.is_dir() {
                    if !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | "target" | "node_modules")
                    ) {
                        directories.push(path);
                    }
                } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "slint") {
                    files.insert(path);
                }
            }
        }
        files
            .into_iter()
            .filter_map(|path| {
                let url = lsp_types::Url::from_file_path(&path).ok()?;
                if open.contains(&url) {
                    return None;
                }
                match std::fs::read_to_string(&path) {
                    Ok(source) => Some((url, Some(source))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some((url, None)),
                    Err(error) => {
                        tracing::debug!("Cannot read {}: {error}", path.display());
                        None
                    }
                }
            })
            .collect::<Vec<_>>()
    })
    .await;
    let sources = match sources {
        Ok(sources) => sources,
        Err(error) => {
            tracing::error!("Workspace reference scan failed: {error}");
            return;
        }
    };
    for (url, source) in &sources {
        if source.as_ref() != cached.get(url)
            && let Err(error) = dc.drop_document(url)
        {
            tracing::debug!("Cannot invalidate {url}: {error}");
        }
    }
    for (url, source) in sources {
        if let Some(source) = source
            && dc.get_document(&url).is_none()
        {
            dc.reload_cached_file(&url, &mut BuildDiagnostics::default()).await;
            if dc.get_document(&url).is_none() {
                let _ = dc.load_url(&url, None, source, &mut BuildDiagnostics::default()).await;
            }
        }
    }
    for url in cached.keys() {
        dc.reload_cached_file(url, &mut BuildDiagnostics::default()).await;
    }
}

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

    async fn request(
        ctx: &mut super::super::Context,
        uri: &Url,
        source: &str,
        anchor: &str,
        include: bool,
    ) -> Vec<Location> {
        let prefix = &source[..source.find(anchor).unwrap()];
        let column = prefix.rsplit('\n').next().unwrap();
        let character = match ctx.session.document_cache.format {
            crate::editor_preview::ByteFormat::Utf8 => column.len(),
            crate::editor_preview::ByteFormat::Utf16 => column.encode_utf16().count(),
        };
        let position = lsp_types::Position::new(
            prefix.bytes().filter(|b| *b == b'\n').count() as u32,
            character as u32,
        );
        let mut handlers = crate::language::RequestHandler::default();
        crate::language::register_request_handlers(&mut handlers);
        let result = handlers.0["textDocument/references"](
            serde_json::json!({
                "textDocument": { "uri": uri }, "position": position,
                "context": { "includeDeclaration": include }
            }),
            ctx,
        )
        .await
        .unwrap();
        serde_json::from_value(result).unwrap()
    }

    #[tokio::test]
    async fn unopened_files_and_unsaved_buffers() {
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.slint");
        let base = Url::from_file_path(&base_path).unwrap();
        let disk = "export component Base { in-out property <int> disk-value; }";
        let editor = "export component Base { in-out property <int> value-name; }";
        std::fs::write(&base_path, disk).unwrap();
        let consumer_path = temp.path().join("consumer.slint");
        let consumer = Url::from_file_path(&consumer_path).unwrap();
        let consumer_text = "import { Base as Renamed } from \"base.slint\"; component Consumer inherits Renamed { value_name: 1; /* 😀 */ x: root.value-name; }";
        std::fs::write(&consumer_path, consumer_text).unwrap();
        std::fs::write(
            temp.path().join("unrelated.slint"),
            "component Other { in property <int> value-name; x: value-name; }",
        )
        .unwrap();
        std::fs::write(temp.path().join("broken.slint"), "component Broken { invalid syntax")
            .unwrap();
        let mut ctx = test::mock_context();
        ctx.init_param.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
            uri: Url::from_directory_path(temp.path()).unwrap(),
            name: "test".into(),
        }]);
        let encoded_base =
            Url::parse(&base.as_str().replace("base.slint", "base.%73lint")).unwrap();
        ctx.session.open_document(editor.into(), encoded_base, Some(7)).await.unwrap();
        assert!(ctx.session.document_cache.get_document(&consumer).is_none());
        let hits = request(&mut ctx, &base, editor, "value-name;", true).await;
        assert_eq!(hits.len(), 3);
        assert_eq!(hits.iter().filter(|l| l.uri == consumer).count(), 2);
        let revision = ctx.session.document_cache.revision();
        assert_eq!(request(&mut ctx, &base, editor, "value-name;", true).await, hits);
        assert_eq!(ctx.session.document_cache.revision(), revision);
        assert_eq!(ctx.session.document_cache.document_version(&base), Some(7));

        ctx.session
            .open_document(
                consumer_text.replace("root.value-name", "42"),
                consumer.clone(),
                Some(8),
            )
            .await
            .unwrap();
        assert_eq!(request(&mut ctx, &base, editor, "value-name;", false).await.len(), 1);
        assert_eq!(std::fs::read_to_string(&base_path).unwrap(), disk);
        assert_eq!(std::fs::read_to_string(&consumer_path).unwrap(), consumer_text);

        ctx.session.close_document(consumer.clone()).await.unwrap();
        assert_eq!(request(&mut ctx, &base, editor, "value-name;", false).await.len(), 2);
        std::fs::write(&consumer_path, consumer_text.replace("root.value-name", "42")).unwrap();
        assert_eq!(request(&mut ctx, &base, editor, "value-name;", false).await.len(), 1);
        std::fs::remove_file(&consumer_path).unwrap();
        assert!(request(&mut ctx, &base, editor, "value-name;", false).await.is_empty());
        std::fs::write(&consumer_path, consumer_text).unwrap();
        assert_eq!(request(&mut ctx, &base, editor, "value-name;", false).await.len(), 2);
        ctx.session
            .trigger_file_watcher(
                consumer.clone(),
                i_slint_live_preview::file_watcher::FileChangeKind::Changed,
            )
            .await
            .unwrap();
        assert_eq!(request(&mut ctx, &base, editor, "value-name;", false).await.len(), 2);
        assert_eq!(ctx.session.document_cache.document_version(&base), Some(7));
    }

    #[tokio::test]
    #[allow(deprecated)] // root_uri remains common among clients without workspace folders.
    async fn root_uri_and_unopened_target() {
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.slint");
        let source = "export component Base { in-out property <int> value; }";
        std::fs::write(&base_path, source).unwrap();
        std::fs::write(
            temp.path().join("use.slint"),
            "import { Base } from \"base.slint\"; component App inherits Base { value: 1; }",
        )
        .unwrap();
        let mut ctx = test::mock_context();
        ctx.init_param.root_uri = Some(Url::from_directory_path(temp.path()).unwrap());
        assert_eq!(
            request(&mut ctx, &Url::from_file_path(base_path).unwrap(), source, "value;", true)
                .await
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn multiple_roots_and_changed_dependency() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let base_path = first.path().join("base.slint");
        let base = Url::from_file_path(&base_path).unwrap();
        let base_source = "export component Base { in-out property <int> value; }";
        std::fs::write(&base_path, base_source).unwrap();
        let bridge_path = first.path().join("bridge.slint");
        std::fs::write(&bridge_path, "export { Base as Public } from \"base.slint\";").unwrap();
        let consumer_path = second.path().join("consumer.slint");
        let consumer_source = format!(
            "import {{ Public as Local }} from \"{}\"; component Consumer inherits Local {{ value: 1; }}",
            bridge_path.display()
        );
        std::fs::write(&consumer_path, &consumer_source).unwrap();
        let consumer = Url::from_file_path(&consumer_path).unwrap();
        let mut ctx = test::mock_context();
        ctx.init_param.workspace_folders = Some(
            [first.path(), second.path(), first.path()]
                .into_iter()
                .map(|root| lsp_types::WorkspaceFolder {
                    uri: Url::from_directory_path(root).unwrap(),
                    name: "test".into(),
                })
                .collect(),
        );
        ctx.session
            .open_document(consumer_source.clone(), consumer.clone(), Some(1))
            .await
            .unwrap();
        assert_eq!(request(&mut ctx, &consumer, &consumer_source, "value:", true).await.len(), 2);
        let shifted = format!("\n\n{base_source}");
        std::fs::write(&base_path, &shifted).unwrap();
        let hits = request(&mut ctx, &consumer, &consumer_source, "value:", true).await;
        assert_eq!(hits.len(), 2);
        let declaration = hits.iter().find(|l| l.uri == base).unwrap();
        assert_eq!(declaration.range.start.line, 2);
        assert_eq!(ctx.session.document_cache.document_version(&consumer), Some(1));
    }

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
    fn lexical_shadowing() {
        let source = r#"
component App {
    in property <int> value-name/*declaration*/;
    function inspect() {
        debug(value-name/*before*/);
        let value-name = value-name/*initializer*/;
        debug(value-name/*local*/);
        debug(root.value_name/*qualified*/);
    }
    function with-argument(value_name: int) { debug(value-name/*argument*/); }
    function nested() {
        if true { let value-name = 3; debug(value-name/*nested-local*/); }
        debug(value-name/*after-block*/);
    }
    x: value-name/*outside*/;
}"#;
        let (dc, uri, _) = test::loaded_document_cache(source.into());
        check(
            &dc,
            &uri,
            source,
            "value-name/*declaration*/",
            &[
                "value-name/*declaration*/",
                "value-name/*before*/",
                "value-name/*initializer*/",
                "value_name/*qualified*/",
                "value-name/*after-block*/",
                "value-name/*outside*/",
            ],
            true,
        );
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
        let response = spin_on::spin_on(handlers.0["textDocument/references"](
            serde_json::json!({
                "textDocument": { "uri": uri }, "position": position,
                "context": { "includeDeclaration": false }
            }),
            &mut ctx,
        ))
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
