// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use i_slint_compiler::diagnostics::{SourceLocation, Spanned};
use i_slint_compiler::element_type::ElementType;
use i_slint_compiler::expression_tree::{Callable, Expression};
use i_slint_compiler::langtype::{EnumerationValue, Struct, Type};
use i_slint_compiler::lookup::{LookupObject, LookupResult, LookupResultCallable};
use i_slint_compiler::namedreference::NamedReference;
use i_slint_compiler::object_tree::ElementRc;
use i_slint_compiler::parser::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, syntax_nodes};
use i_slint_compiler::source_path::SourcePath;
use smol_str::{SmolStr, ToSmolStr};
use std::{rc::Rc, sync::Arc};

#[derive(Clone, Debug)]
pub enum TokenInfo {
    Type(Type),
    ElementType(ElementType),
    ElementRc(ElementRc),
    NamedReference(NamedReference),
    EnumerationValue(EnumerationValue),
    FileName(SourcePath),
    Image(SourcePath),
    LocalProperty(syntax_nodes::PropertyDeclaration),
    LocalCallback(syntax_nodes::CallbackDeclaration),
    LocalFunction(syntax_nodes::Function),
    /// This is like a NamedReference, but the element doesn't have an ElementRc because
    /// its enclosing component might not have been properly parsed
    IncompleteNamedReference(ElementType, SmolStr),
    /// A field of a struct, and the struct it belongs to
    StructField(Arc<Struct>, SmolStr),
    /// The model data of a `for`, named by its declared identifier
    ModelData(ElementRc),
}

/// Resolve a struct/enum declaration reference to its syntax node using the
/// documents the language server already keeps loaded, so the compiled type does
/// not need to carry a syntax tree.
pub fn node_for_decl(
    document_cache: &crate::DocumentCache,
    decl: &SourceLocation,
) -> Option<SyntaxNode> {
    let source_file = decl.source_file.as_ref()?;
    let doc = document_cache.get_document_for_source_file(source_file)?;
    let node = doc.node.as_ref()?.covering_element(decl_range(decl)).into_node()?;
    Some(SyntaxNode { node, source_file: source_file.clone() })
}

fn decl_range(decl: &SourceLocation) -> TextRange {
    TextRange::at((decl.span.offset as u32).into(), (decl.span.length as u32).into())
}

impl TokenInfo {
    pub(crate) fn is_same_symbol(
        &self,
        document_cache: &crate::DocumentCache,
        other: &Self,
    ) -> bool {
        if let (Some(left), Some(right)) =
            (self.declaration(document_cache), other.declaration(document_cache))
        {
            return left.source_file.path() == right.source_file.path()
                && left.text_range() == right.text_range();
        }
        fn check_element(
            element: &i_slint_compiler::object_tree::ElementRc,
            node: &SyntaxNode,
        ) -> bool {
            if element.borrow().debug.iter().any(|di| {
                Arc::ptr_eq(&di.node.source_file, &node.source_file)
                    && di.node.text_range() == node.text_range()
            }) {
                return true;
            }
            if let i_slint_compiler::element_type::ElementType::Component(c) =
                &element.borrow().base_type
            {
                check_element(&c.root_element, node)
            } else {
                false
            }
        }

        match (self, other) {
            (crate::token_info::TokenInfo::Type(s), crate::token_info::TokenInfo::Type(o)) => {
                s == o
            }
            (
                crate::token_info::TokenInfo::ElementType(s),
                crate::token_info::TokenInfo::ElementType(o),
            ) => s == o,
            (
                crate::token_info::TokenInfo::ElementRc(s),
                crate::token_info::TokenInfo::ElementRc(o),
            ) => Rc::ptr_eq(s, o),
            (
                crate::token_info::TokenInfo::LocalProperty(s),
                crate::token_info::TokenInfo::LocalProperty(o),
            ) => Arc::ptr_eq(&s.source_file, &o.source_file) && s.text_range() == o.text_range(),
            (
                crate::token_info::TokenInfo::NamedReference(nl),
                crate::token_info::TokenInfo::NamedReference(nr),
            ) => Rc::ptr_eq(&nl.element(), &nr.element()) && nl.name() == nr.name(),
            (
                crate::token_info::TokenInfo::ElementType(
                    i_slint_compiler::element_type::ElementType::Component(c),
                ),
                crate::token_info::TokenInfo::ElementRc(e),
            )
            | (
                crate::token_info::TokenInfo::ElementRc(e),
                crate::token_info::TokenInfo::ElementType(
                    i_slint_compiler::element_type::ElementType::Component(c),
                ),
            ) => {
                if let Some(ce) = c.node.as_ref().map(|cn| cn.Element()) {
                    e.borrow().debug.iter().any(|di| {
                        Some(di.node.source_file.path()) == ce.source_file().map(|sf| sf.path())
                            && di.node.text_range() == ce.text_range()
                    })
                } else {
                    false
                }
            }
            (
                crate::token_info::TokenInfo::NamedReference(nr),
                crate::token_info::TokenInfo::LocalProperty(s),
            )
            | (
                crate::token_info::TokenInfo::LocalProperty(s),
                crate::token_info::TokenInfo::NamedReference(nr),
            ) => named_reference_resolves_to(nr, s),
            (
                crate::token_info::TokenInfo::LocalProperty(s),
                crate::token_info::TokenInfo::IncompleteNamedReference(nr1, nr2),
            )
            | (
                crate::token_info::TokenInfo::IncompleteNamedReference(nr1, nr2),
                crate::token_info::TokenInfo::LocalProperty(s),
            ) => {
                matches!(nr1, i_slint_compiler::element_type::ElementType::Component(c) if s.parent().is_some_and(|n| check_element(&c.root_element, &n)))
                    && Some(nr2)
                        == i_slint_compiler::parser::identifier_text(&s.DeclaredIdentifier())
                            .as_ref()
            }
            (
                crate::token_info::TokenInfo::LocalCallback(s),
                crate::token_info::TokenInfo::LocalCallback(o),
            ) => Arc::ptr_eq(&s.source_file, &o.source_file) && s.text_range() == o.text_range(),
            (
                crate::token_info::TokenInfo::NamedReference(nr),
                crate::token_info::TokenInfo::LocalCallback(s),
            )
            | (
                crate::token_info::TokenInfo::LocalCallback(s),
                crate::token_info::TokenInfo::NamedReference(nr),
            ) => named_reference_resolves_to(nr, s),
            (
                crate::token_info::TokenInfo::LocalCallback(s),
                crate::token_info::TokenInfo::IncompleteNamedReference(nr1, nr2),
            )
            | (
                crate::token_info::TokenInfo::IncompleteNamedReference(nr1, nr2),
                crate::token_info::TokenInfo::LocalCallback(s),
            ) => {
                matches!(nr1, i_slint_compiler::element_type::ElementType::Component(c) if s.parent().is_some_and(|n| check_element(&c.root_element, &n)))
                    && Some(nr2)
                        == i_slint_compiler::parser::identifier_text(&s.DeclaredIdentifier())
                            .as_ref()
            }
            (
                crate::token_info::TokenInfo::LocalFunction(s),
                crate::token_info::TokenInfo::LocalFunction(o),
            ) => Arc::ptr_eq(&s.source_file, &o.source_file) && s.text_range() == o.text_range(),
            (
                crate::token_info::TokenInfo::NamedReference(nr),
                crate::token_info::TokenInfo::LocalFunction(s),
            )
            | (
                crate::token_info::TokenInfo::LocalFunction(s),
                crate::token_info::TokenInfo::NamedReference(nr),
            ) => named_reference_resolves_to(nr, s),
            (
                crate::token_info::TokenInfo::LocalFunction(s),
                crate::token_info::TokenInfo::IncompleteNamedReference(nr1, nr2),
            )
            | (
                crate::token_info::TokenInfo::IncompleteNamedReference(nr1, nr2),
                crate::token_info::TokenInfo::LocalFunction(s),
            ) => {
                matches!(nr1, i_slint_compiler::element_type::ElementType::Component(c) if s.parent().is_some_and(|n| check_element(&c.root_element, &n)))
                    && Some(nr2)
                        == i_slint_compiler::parser::identifier_text(&s.DeclaredIdentifier())
                            .as_ref()
            }
            (_, _) => false,
        }
    }

    /// Returns the node to the declaration of what the token represents, if it exists
    pub fn declaration(&self, document_cache: &crate::DocumentCache) -> Option<SyntaxNode> {
        match self {
            TokenInfo::Type(ty) => match ty {
                Type::Struct(s) => s.node().and_then(|n| node_for_decl(document_cache, n)),
                Type::Enumeration(e) => {
                    e.node.as_ref().and_then(|n| node_for_decl(document_cache, n))
                }
                _ => None,
            },

            TokenInfo::ElementType(el) => {
                if let ElementType::Component(c) = el {
                    Some(c.root_element.borrow().debug.first()?.node.clone().into())
                } else {
                    None
                }
            }
            TokenInfo::ElementRc(el) => Some(el.borrow().debug.first()?.node.clone().into()),
            TokenInfo::NamedReference(nr) => {
                nr.element().borrow().property_declaration_node(nr.name())
            }

            TokenInfo::EnumerationValue(v) => {
                let enum_node = i_slint_compiler::parser::syntax_nodes::EnumDeclaration::from(
                    node_for_decl(document_cache, v.enumeration.node.as_ref()?)?,
                );
                enum_node.EnumValue().nth(v.value).map(|x| x.clone().into())
            }

            TokenInfo::FileName(_) | TokenInfo::Image(_) => None,

            TokenInfo::LocalProperty(x) => Some(x.clone().into()),
            TokenInfo::LocalCallback(x) => Some(x.clone().into()),
            TokenInfo::LocalFunction(x) => Some(x.clone().into()),
            TokenInfo::StructField(s, field) => {
                let decl = node_for_decl(document_cache, s.node()?)?;
                syntax_nodes::StructDeclaration::new(decl)?
                    .ObjectType()
                    .ObjectTypeMember()
                    .find(|m| {
                        m.child_token(SyntaxKind::Identifier).is_some_and(|t| {
                            i_slint_compiler::parser::normalize_identifier(t.text()) == *field
                        })
                    })
                    .map(|m| m.into())
            }
            TokenInfo::ModelData(elem) => {
                let node = elem.borrow().debug.first()?.node.clone();
                let repeated = node.parent()?.parent()?;
                syntax_nodes::RepeatedElement::new(repeated)?.DeclaredIdentifier().map(|d| d.into())
            }
            TokenInfo::IncompleteNamedReference(element_type, prop_name) => {
                let mut element_type = element_type.clone();
                while let ElementType::Component(com) | ElementType::Interface(Some(com)) =
                    element_type
                {
                    if let Some((_, p)) = com.root_element.borrow().declaration(prop_name) {
                        return p.node.clone();
                    }
                    element_type = com.root_element.borrow().base_type.clone();
                }
                None
            }
        }
    }
}

/// Map what a name resolved to onto the thing the editor can show or jump to
fn token_info_from_lookup_result(lr: LookupResult) -> Option<TokenInfo> {
    match lr {
        LookupResult::Expression { expression: Expression::ElementReference(e), .. } => {
            Some(TokenInfo::ElementRc(e.upgrade()?))
        }
        LookupResult::Expression { expression: Expression::PropertyReference(nr), .. } => {
            Some(TokenInfo::NamedReference(nr))
        }
        LookupResult::Expression { expression: Expression::EnumerationValue(v), .. } => {
            Some(TokenInfo::EnumerationValue(v))
        }
        LookupResult::Expression {
            expression: Expression::StructFieldAccess { base, name },
            ..
        } => match base.ty() {
            Type::Struct(s) => Some(TokenInfo::StructField(s, name)),
            _ => None,
        },
        LookupResult::Expression {
            expression: Expression::RepeaterModelReference { element },
            ..
        } => Some(TokenInfo::ModelData(element.upgrade()?)),
        LookupResult::Enumeration(e) => Some(TokenInfo::Type(Type::Enumeration(e))),
        LookupResult::Callable(LookupResultCallable::Callable(
            Callable::Callback(nr) | Callable::Function(nr),
        )) => Some(TokenInfo::NamedReference(nr)),
        _ => None,
    }
}

pub fn token_info(document_cache: &crate::DocumentCache, token: SyntaxToken) -> Option<TokenInfo> {
    let mut node = token.parent();
    if node.kind() == SyntaxKind::SubElement && token.kind() == SyntaxKind::Identifier {
        let url = token.source_file.path().to_url()?;
        let element = document_cache.element_at_offset(&url, token.text_range().start())?;
        return Some(TokenInfo::ElementRc(element.element));
    }
    if matches!(node.kind(), SyntaxKind::ObjectTypeMember | SyntaxKind::EnumValue)
        && token.kind() == SyntaxKind::Identifier
    {
        let declaration = if node.kind() == SyntaxKind::EnumValue {
            node.parent()?
        } else {
            node.parent()?.parent()?
        };
        let doc = document_cache.get_document_for_source_file(&node.source_file)?;
        let name = i_slint_compiler::parser::identifier_text(
            &declaration.child_node(SyntaxKind::DeclaredIdentifier)?,
        )?;
        let ty = doc.local_registry.lookup(&name);
        let member = i_slint_compiler::parser::normalize_identifier(token.text());
        return match ty {
            Type::Struct(s) => Some(TokenInfo::StructField(s, member)),
            Type::Enumeration(e) => {
                let value = e.values.iter().position(|name| *name == member)?;
                Some(TokenInfo::EnumerationValue(EnumerationValue { enumeration: e, value }))
            }
            _ => None,
        };
    }
    if node.kind() == SyntaxKind::AtImageUrl && token.kind() == SyntaxKind::StringLiteral {
        let path = i_slint_compiler::literals::unescape_string(token.text())?;
        return Some(TokenInfo::Image(token.source_file.path().parent().join(&path)?));
    }

    loop {
        if let Some(n) = syntax_nodes::QualifiedName::new(node.clone()) {
            let parent = n.parent()?;
            return match parent.kind() {
                SyntaxKind::Type => {
                    let qual = i_slint_compiler::object_tree::QualifiedTypeName::from_node(n);
                    let doc = document_cache.get_document_for_source_file(&node.source_file)?;
                    Some(TokenInfo::Type(doc.local_registry.lookup_qualified(&qual.members)))
                }
                SyntaxKind::Element => {
                    if !crate::is_element_node_ignored(&parent.into()) {
                        let qual = i_slint_compiler::object_tree::QualifiedTypeName::from_node(n);
                        let doc = document_cache.get_document_for_source_file(&node.source_file)?;
                        Some(TokenInfo::ElementType(
                            doc.local_registry.lookup_element(&qual.to_string()).ok()?,
                        ))
                    } else {
                        None
                    }
                }
                SyntaxKind::Expression
                | SyntaxKind::StatePropertyChange
                | SyntaxKind::PropertyAnimation => {
                    if token.kind() != SyntaxKind::Identifier {
                        return None;
                    }
                    let lr = crate::util::with_lookup_ctx(
                        document_cache,
                        node,
                        Some(token.text_range().start()),
                        |ctx| {
                            let mut it = n
                                .children_with_tokens()
                                .filter_map(|t| t.into_token())
                                .filter(|t| t.kind() == SyntaxKind::Identifier);
                            let mut cur_tok = it.next()?;
                            let first_str =
                                i_slint_compiler::parser::normalize_identifier(cur_tok.text());
                            let global = i_slint_compiler::lookup::global_lookup();
                            let mut expr_it = global.lookup(ctx, &first_str)?;
                            while cur_tok.token != token.token {
                                cur_tok = it.next()?;
                                let str =
                                    i_slint_compiler::parser::normalize_identifier(cur_tok.text());
                                expr_it = expr_it.lookup(ctx, &str)?;
                            }
                            Some(expr_it)
                        },
                    )?;
                    token_info_from_lookup_result(lr?)
                }
                _ => None,
            };
        } else if let Some(n) = syntax_nodes::MemberAccess::new(node.clone()) {
            // Member access on something that isn't a plain name, such as `foo[0].bar`
            if token.kind() != SyntaxKind::Identifier {
                return None;
            }
            let name = i_slint_compiler::parser::normalize_identifier(token.text());
            let lr = crate::util::with_lookup_ctx(
                document_cache,
                node.clone(),
                Some(token.text_range().start()),
                |ctx| Expression::from_expression_node(n.Expression(), ctx).lookup(ctx, &name),
            )?;
            return token_info_from_lookup_result(lr?);
        } else if let Some(n) = syntax_nodes::ImportIdentifier::new(node.clone()) {
            let doc = document_cache.get_document_for_source_file(&node.source_file)?;
            let imp_name = i_slint_compiler::typeloader::ImportedName::from_node(n);
            return doc
                .local_registry
                .lookup_element(&imp_name.internal_name)
                .ok()
                .map(TokenInfo::ElementType)
                .or_else(|| {
                    let ty = doc.local_registry.lookup(&imp_name.internal_name);
                    (ty != Type::Invalid).then_some(TokenInfo::Type(ty))
                });
        } else if let Some(n) = syntax_nodes::ExportSpecifier::new(node.clone()) {
            let doc = document_cache.get_document_for_source_file(&node.source_file)?;
            let (_, exp) = i_slint_compiler::object_tree::ExportedName::from_export_specifier(&n);
            return match doc.exports.find(exp.as_str())? {
                itertools::Either::Left(c) => {
                    Some(TokenInfo::ElementType(ElementType::Component(c)))
                }
                itertools::Either::Right(ty) => Some(TokenInfo::Type(ty)),
            };
        } else if matches!(node.kind(), SyntaxKind::ImportSpecifier | SyntaxKind::ExportModule) {
            let import_text =
                node.child_text(SyntaxKind::StringLiteral)?.trim_matches('\"').to_string();
            let library_file = import_text
                .starts_with('@')
                .then(|| {
                    document_cache.resolve_import_path(Some(&token.clone().into()), &import_text)
                })
                .flatten();
            let import_file = match library_file {
                Some(path) => path,
                None => node.source_file.path().parent().join(&import_text)?,
            };
            return Some(TokenInfo::FileName(import_file));
        } else if syntax_nodes::BindingExpression::new(node.clone()).is_some() {
            // don't fallback to the Binding
            return None;
        } else if let Some(n) = syntax_nodes::Binding::new(node.clone()) {
            if token.kind() != SyntaxKind::Identifier {
                return None;
            }
            let prop_name = i_slint_compiler::parser::normalize_identifier(token.text());
            let element = syntax_nodes::Element::new(n.parent()?)?;
            if let Some(p) = element.PropertyDeclaration().find_map(|p| {
                (i_slint_compiler::parser::identifier_text(&p.DeclaredIdentifier())? == prop_name)
                    .then_some(p)
            }) {
                return Some(TokenInfo::LocalProperty(p));
            }
            return find_property_declaration_in_base(document_cache, element, &prop_name);
        } else if let Some(n) = syntax_nodes::TwoWayBinding::new(node.clone()) {
            if token.kind() != SyntaxKind::Identifier {
                return None;
            }
            let prop_name = i_slint_compiler::parser::normalize_identifier(token.text());
            if prop_name != i_slint_compiler::parser::identifier_text(&n)? {
                return None;
            }
            let element = syntax_nodes::Element::new(n.parent()?)?;
            if let Some(p) = element.PropertyDeclaration().find_map(|p| {
                (i_slint_compiler::parser::identifier_text(&p.DeclaredIdentifier())? == prop_name)
                    .then_some(p)
            }) {
                return Some(TokenInfo::LocalProperty(p));
            }
            return find_property_declaration_in_base(document_cache, element, &prop_name);
        } else if let Some(n) = syntax_nodes::CallbackConnection::new(node.clone()) {
            if token.kind() != SyntaxKind::Identifier {
                return None;
            }
            let prop_name = i_slint_compiler::parser::normalize_identifier(token.text());
            if prop_name != i_slint_compiler::parser::identifier_text(&n)? {
                return None;
            }
            let element = syntax_nodes::Element::new(n.parent()?)?;
            if let Some(p) = element.CallbackDeclaration().find_map(|p| {
                (i_slint_compiler::parser::identifier_text(&p.DeclaredIdentifier())? == prop_name)
                    .then_some(p)
            }) {
                return Some(TokenInfo::LocalCallback(p));
            }
            return find_property_declaration_in_base(document_cache, element, &prop_name);
        } else if node.kind() == SyntaxKind::DeclaredIdentifier {
            if token.kind() != SyntaxKind::Identifier {
                return None;
            }
            let parent = node.parent()?;
            if matches!(
                parent.kind(),
                SyntaxKind::PropertyChangedCallback | SyntaxKind::PropertyDeclaration
            ) {
                let prop_name = i_slint_compiler::parser::normalize_identifier(token.text());
                let element = syntax_nodes::Element::new(parent.parent()?)?;
                if let Some(p) = element.PropertyDeclaration().find_map(|p| {
                    (i_slint_compiler::parser::identifier_text(&p.DeclaredIdentifier())?
                        == prop_name)
                        .then_some(p)
                }) {
                    return Some(TokenInfo::LocalProperty(p));
                }
                return find_property_declaration_in_base(document_cache, element, &prop_name);
            }
            if parent.kind() == SyntaxKind::CallbackDeclaration {
                return Some(TokenInfo::LocalCallback(parent.into()));
            }
            if parent.kind() == SyntaxKind::Function {
                return Some(TokenInfo::LocalFunction(parent.into()));
            }
            if parent.kind() == SyntaxKind::Component {
                let doc = document_cache.get_document_for_source_file(&node.source_file)?;
                let element_type = doc
                    .local_registry
                    .lookup_element(
                        i_slint_compiler::parser::normalize_identifier(token.text()).as_str(),
                    )
                    .ok()?;
                if let ElementType::Component(component) = &element_type
                    && component
                        .node
                        .as_ref()
                        .map(|n| n.text_range().contains_range(token.text_range()))
                        .unwrap_or_default()
                {
                    return Some(TokenInfo::ElementType(element_type));
                }
            }
            if parent.kind() == SyntaxKind::StructDeclaration {
                let doc = document_cache.get_document_for_source_file(&node.source_file)?;
                let ty = doc
                    .local_registry
                    .lookup(i_slint_compiler::parser::normalize_identifier(token.text()).as_str());
                match &ty {
                    Type::Struct(s)
                        if s.node()
                            .is_some_and(|n| decl_range(n).contains_range(token.text_range())) =>
                    {
                        return Some(TokenInfo::Type(ty));
                    }
                    _ => { /* nothing to do */ }
                }
            }
            if parent.kind() == SyntaxKind::EnumDeclaration {
                let doc = document_cache.get_document_for_source_file(&node.source_file)?;
                let ty = doc
                    .local_registry
                    .lookup(i_slint_compiler::parser::normalize_identifier(token.text()).as_str());
                match &ty {
                    Type::Enumeration(e)
                        if e.node
                            .as_ref()
                            .is_some_and(|n| decl_range(n).contains_range(token.text_range())) =>
                    {
                        return Some(TokenInfo::Type(ty));
                    }
                    _ => { /* nothing to do */ }
                }
            }
            return None;
        }
        node = node.parent()?;
    }
}

pub fn symbol_occurrences(
    document_cache: &crate::DocumentCache,
    symbol: &TokenInfo,
    document: &syntax_nodes::Document,
) -> Vec<SyntaxToken> {
    let Some(declaration) = symbol.declaration(document_cache) else { return Vec::new() };
    let identifier = declaration
        .child_node(SyntaxKind::DeclaredIdentifier)
        .and_then(|id| id.child_token(SyntaxKind::Identifier))
        .or_else(|| declaration.child_token(SyntaxKind::Identifier))
        .or_else(|| {
            let parent = declaration.parent()?;
            parent
                .child_node(SyntaxKind::DeclaredIdentifier)
                .and_then(|id| id.child_token(SyntaxKind::Identifier))
                .or_else(|| parent.child_token(SyntaxKind::Identifier))
        });
    let Some(identifier) = identifier else { return Vec::new() };
    let mut names =
        std::collections::HashSet::from([i_slint_compiler::parser::normalize_identifier(
            identifier.text(),
        )]);
    for import in document.ImportSpecifier().filter_map(|i| i.ImportIdentifierList()) {
        for id in import.ImportIdentifier() {
            let external = id.ExternalName().child_token(SyntaxKind::Identifier);
            if let Some(external) = external
                && token_info(document_cache, external.clone())
                    .is_some_and(|info| symbol.is_same_symbol(document_cache, &info))
            {
                names.insert(i_slint_compiler::parser::normalize_identifier(external.text()));
                if let Some(internal) =
                    id.InternalName().and_then(|n| n.child_token(SyntaxKind::Identifier))
                {
                    names.insert(i_slint_compiler::parser::normalize_identifier(internal.text()));
                }
            }
        }
    }
    for export in document.ExportsList() {
        for specifier in export.ExportSpecifier() {
            for node in specifier.children() {
                if let Some(token) = node.child_token(SyntaxKind::Identifier)
                    && token_info(document_cache, token.clone())
                        .is_some_and(|info| symbol.is_same_symbol(document_cache, &info))
                {
                    names.insert(i_slint_compiler::parser::normalize_identifier(token.text()));
                }
            }
        }
    }
    let mut occurrences = Vec::new();
    let mut token = document.first_token();
    while let Some(current) = token {
        token = current.next_token();
        if current.kind() == SyntaxKind::Identifier
            && names.contains(&i_slint_compiler::parser::normalize_identifier(current.text()))
            && let Some(info) = token_info(document_cache, current.clone())
            && symbol.is_same_symbol(document_cache, &info)
        {
            occurrences.push(current);
        }
    }
    occurrences
}

/// Try to lookup the property `prop_name` in the base of the given Element
fn find_property_declaration_in_base(
    document_cache: &crate::DocumentCache,
    element: syntax_nodes::Element,
    prop_name: &str,
) -> Option<TokenInfo> {
    let global_tr = document_cache.global_type_registry();
    let tr = element
        .source_file()
        .and_then(|sf| document_cache.get_document_for_source_file(sf))
        .map(|doc| &doc.local_registry)
        .unwrap_or(&global_tr);

    let element_type = crate::util::lookup_current_element_type((*element).clone(), tr)?;
    Some(TokenInfo::IncompleteNamedReference(element_type, prop_name.to_smolstr()))
}

/// Whether `named_reference` resolves to the member declared at `declaration`. The reference's name
/// is the key the member is stored under - for a declaration that shadows an inherited member that is
/// a mangled internal name - so comparing the resolved node matches the shadowing declaration even
/// though that key differs from the source name.
fn named_reference_resolves_to(
    named_reference: &i_slint_compiler::namedreference::NamedReference,
    declaration: &SyntaxNode,
) -> bool {
    named_reference
        .element()
        .borrow()
        .property_declaration_node(named_reference.name())
        .is_some_and(|node| {
            Arc::ptr_eq(&node.source_file, &declaration.source_file)
                && node.text_range() == declaration.text_range()
        })
}
