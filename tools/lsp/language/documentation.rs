// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use i_slint_compiler::parser::{SyntaxKind, SyntaxNode, TextRange};

fn declaration_node(node: &SyntaxNode) -> SyntaxNode {
    let node = if node.kind() == SyntaxKind::Element {
        node.parent()
            .filter(|parent| {
                matches!(parent.kind(), SyntaxKind::Component | SyntaxKind::SubElement)
            })
            .unwrap_or_else(|| node.clone())
    } else {
        node.clone()
    };
    node.parent().filter(|parent| parent.kind() == SyntaxKind::ExportsList).unwrap_or(node)
}

pub(super) fn documentation_range(node: &SyntaxNode) -> Option<TextRange> {
    let mut token = declaration_node(node).first_token()?.prev_token()?;
    let mut range: Option<TextRange> = None;
    loop {
        if token.kind() != SyntaxKind::Whitespace
            || token.text().bytes().filter(|byte| *byte == b'\n').take(2).count() != 1
        {
            break;
        }
        let Some(comment) = token.prev_token().filter(|token| token.kind() == SyntaxKind::Comment)
        else {
            break;
        };
        let preceding = comment.prev_token();
        if preceding.as_ref().is_some_and(|token| {
            token.kind() != SyntaxKind::Whitespace
                || (!token.text().contains('\n') && token.prev_token().is_some())
        }) {
            break;
        }
        range = Some(range.map_or(comment.text_range(), |range| comment.text_range().cover(range)));
        let Some(previous) = preceding else { break };
        token = previous;
    }
    range
}

pub(super) fn declaration_range(node: &SyntaxNode) -> TextRange {
    let node = declaration_node(node);
    let range = match node.kind() {
        SyntaxKind::ObjectTypeMember | SyntaxKind::EnumValue => {
            crate::util::node_range_without_trailing_ws(&node)
        }
        _ => node.text_range(),
    };
    TextRange::new(
        documentation_range(&node).map_or(range.start(), |docs| docs.start()),
        range.end(),
    )
}
