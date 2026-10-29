// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use crate::editor_preview::{
    DocumentCache,
    token_info::{TokenInfo, token_info},
};
use i_slint_compiler::{element_type::ElementType, parser::SyntaxToken};
use lsp_types::Location;

pub fn implementations(
    document_cache: &DocumentCache,
    token: SyntaxToken,
) -> Option<Vec<Location>> {
    let TokenInfo::ElementType(target) = token_info(document_cache, token)? else { return None };
    if let ElementType::Component(component) = &target
        && (component.is_global() || component.is_interface())
    {
        return None;
    }
    let mut locations = Vec::new();
    for document in document_cache.all_documents() {
        for component in &document.inner_components {
            if component.is_global() || component.is_interface() {
                continue;
            }
            let derived = component
                .root_element
                .borrow()
                .any_in_inheritance_chain(|element| element.base_type == target);
            if derived
                && let Some(node) = &component.node
                && let Some((uri, range)) = crate::util::node_to_url_and_lsp_range(
                    &node.DeclaredIdentifier(),
                    document_cache.format,
                )
                && uri.scheme() != "builtin"
            {
                locations.push(Location::new(uri, range));
            }
        }
    }
    locations.sort_by(|a, b| a.uri.cmp(&b.uri).then(a.range.start.cmp(&b.range.start)));
    locations.dedup();
    Some(locations)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::editor_preview::ByteFormat;
    use crate::language::{Context, LspError, RequestHandler, register_request_handlers, test};
    use lsp_types::{Position, Range, Url};

    async fn request(
        ctx: &mut Context,
        uri: &Url,
        position: Position,
    ) -> Result<Option<Vec<Location>>, LspError> {
        let mut handlers = RequestHandler::default();
        register_request_handlers(&mut handlers);
        let result = handlers.0["textDocument/implementation"](
            serde_json::json!({"textDocument": {"uri": uri}, "position": position}),
            ctx,
        )
        .await?;
        Ok(serde_json::from_value(result).unwrap())
    }

    #[tokio::test]
    async fn derived_components_follow_workspace_changes() {
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.slint");
        let base = Url::from_file_path(&base_path).unwrap();
        let base_source = "export component Base-Widget { in property <int> value; }";
        std::fs::write(&base_path, base_source).unwrap();
        std::fs::write(
            temp.path().join("bridge.slint"),
            "export { Base_Widget as Public } from \"base.slint\";",
        )
        .unwrap();
        let derived_path = temp.path().join("derived.slint");
        let derived = Url::from_file_path(&derived_path).unwrap();
        let derived_source = "import { Public as Local } from \"bridge.slint\";\n/* 😀 */ export component Derived inherits Local {}";
        std::fs::write(&derived_path, derived_source).unwrap();
        let leaf_path = temp.path().join("leaf.slint");
        let leaf = Url::from_file_path(&leaf_path).unwrap();
        let leaf_source = "import { Derived as Parent } from \"derived.slint\";\nexport component Leaf inherits Parent {}";
        std::fs::write(&leaf_path, leaf_source).unwrap();
        std::fs::write(
            temp.path().join("other.slint"),
            "component Base-Widget {} component Other inherits Base_Widget {}",
        )
        .unwrap();
        let mut ctx = test::mock_context();
        ctx.init_param.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
            uri: Url::from_directory_path(temp.path()).unwrap(),
            name: "test".into(),
        }]);
        ctx.session.open_document(base_source.into(), base.clone(), Some(7)).await.unwrap();
        let base_position = Position::new(0, 17);
        assert!(ctx.session.document_cache.get_document(&derived).is_none());
        for format in [ByteFormat::Utf8, ByteFormat::Utf16] {
            ctx.session.document_cache.format = format;
            let prefix = "/* 😀 */ export component ";
            let column = match format {
                ByteFormat::Utf8 => prefix.len(),
                ByteFormat::Utf16 => prefix.encode_utf16().count(),
            } as u32;
            let expected = vec![
                Location::new(
                    derived.clone(),
                    Range::new(Position::new(1, column), Position::new(1, column + 7)),
                ),
                Location::new(leaf.clone(), Range::new(Position::new(1, 17), Position::new(1, 21))),
            ];
            let actual = request(&mut ctx, &base, base_position).await.unwrap().unwrap();
            assert_eq!(actual, expected);
            assert_eq!(
                request(&mut ctx, &derived, Position::new(0, 19)).await.unwrap().unwrap(),
                expected
            );
            assert_eq!(
                request(&mut ctx, &derived, expected[0].range.start).await.unwrap(),
                Some(vec![expected[1].clone()])
            );
            assert_eq!(
                request(&mut ctx, &leaf, expected[1].range.start).await.unwrap(),
                Some(vec![])
            );
            assert_eq!(
                request(
                    &mut ctx,
                    &base,
                    Position::new(0, base_source.find("value").unwrap() as u32)
                )
                .await
                .unwrap(),
                None
            );
        }
        ctx.session
            .open_document(
                derived_source.replace("inherits Local", "inherits Rectangle"),
                derived.clone(),
                Some(8),
            )
            .await
            .unwrap();
        assert_eq!(request(&mut ctx, &base, base_position).await.unwrap(), Some(vec![]));
        assert_eq!(std::fs::read_to_string(&derived_path).unwrap(), derived_source);
        assert_eq!(ctx.session.document_cache.document_version(&derived), Some(8));
        ctx.session.close_document(derived.clone()).await.unwrap();
        assert_eq!(request(&mut ctx, &base, base_position).await.unwrap().unwrap().len(), 2);
        std::fs::write(
            &derived_path,
            derived_source.replace("inherits Local", "inherits Rectangle"),
        )
        .unwrap();
        assert_eq!(request(&mut ctx, &base, base_position).await.unwrap(), Some(vec![]));
        std::fs::write(&derived_path, derived_source).unwrap();
        std::fs::remove_file(&leaf_path).unwrap();
        assert_eq!(request(&mut ctx, &base, base_position).await.unwrap().unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(&base_path).unwrap(), base_source);
        let invalid_path = temp.path().join("encoded.slint");
        let invalid: Vec<_> = [0xff, 0xfe]
            .into_iter()
            .chain("component Encoded {}".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        std::fs::write(&invalid_path, &invalid).unwrap();
        let error = request(&mut ctx, &base, base_position).await.unwrap_err();
        assert!(error.message.contains("encoded.slint"), "{error}");
        assert_eq!(std::fs::read(&invalid_path).unwrap(), invalid);
    }
}
