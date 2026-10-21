#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Every HTTP operation must be registered through `routes!` so the OpenAPI
//! document, and with it the authorization matrix check, sees it. This test
//! parses the production source and fails on any other way of registering a
//! route: `.route`, `.route_service`, `.nest`, `.nest_service`, `.fallback*`,
//! a `.merge` of anything but the known OpenAPI sub-routers, or an axum
//! method-router constructor (`get(..)`, `post(..)`, ...) outside the one
//! `/openapi.json` route. Test-only items (`#[cfg(test)]`) are skipped; code
//! after them is not.
use syn::{
    Attribute, Expr, ExprCall, ExprMethodCall, ImplItemFn, ItemConst, ItemFn, ItemImpl, ItemMod,
    ItemStatic, Lit,
    visit::{self, Visit},
};

const REGISTERING_METHODS: [&str; 7] = [
    "route",
    "route_service",
    "nest",
    "nest_service",
    "fallback",
    "fallback_service",
    "fallback_service_with",
];

const METHOD_ROUTER_CONSTRUCTORS: [&str; 18] = [
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "head",
    "options",
    "trace",
    "any",
    "on",
    "get_service",
    "post_service",
    "put_service",
    "patch_service",
    "delete_service",
    "any_service",
    "on_service",
    "connect",
];

const MACRO_NEEDLES: [&str; 6] = [
    ".route(",
    ".nest(",
    ".fallback(",
    "route_service",
    "nest_service",
    ".merge(",
];

/// The OpenAPI sub-routers `openapi_router` and `workflows::router` merge.
const KNOWN_MERGES: [&str; 3] = ["uploads", "hooks", "router"];

fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Ident>()
                .is_ok_and(|ident| ident == "test")
    })
}

struct Scan {
    found: Vec<String>,
}

fn first_string_arg(call: &ExprMethodCall) -> Option<String> {
    match call.args.first()? {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(text) => Some(text.value()),
            _ => None,
        },
        _ => None,
    }
}

fn merge_is_known(call: &ExprMethodCall) -> bool {
    match call.args.first() {
        Some(Expr::Path(path)) => path
            .path
            .get_ident()
            .is_some_and(|ident| KNOWN_MERGES.contains(&ident.to_string().as_str())),
        Some(Expr::Call(call)) => {
            match &*call.func {
                Expr::Path(path) => path.path.segments.last().is_some_and(|segment| {
                    KNOWN_MERGES.contains(&segment.ident.to_string().as_str())
                }),
                _ => false,
            }
        }
        _ => false,
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_fn(self, node);
        }
    }
    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_impl(self, node);
        }
    }
    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_impl_item_fn(self, node);
        }
    }
    fn visit_item_const(&mut self, node: &'ast ItemConst) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_const(self, node);
        }
    }
    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_static(self, node);
        }
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let name = node.method.to_string();
        if name == "route" && first_string_arg(node).as_deref() == Some("/openapi.json") {
            // The one sanctioned registration. Its method-router argument is
            // not scanned, but the receiver is.
            visit::visit_expr(self, &node.receiver);
            return;
        }
        if REGISTERING_METHODS.contains(&name.as_str()) {
            self.found.push(format!(".{name}(..)"));
        } else if name == "merge" && !merge_is_known(node) {
            self.found
                .push(".merge(<router built elsewhere>)".to_owned());
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // `syn` does not parse macro bodies, so read their tokens as text. The
        // utoipa `routes!` macro is the sanctioned way to name handlers.
        let name = node
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if name != "routes" {
            let text = node.tokens.to_string().replace(' ', "");
            for needle in MACRO_NEEDLES {
                if text.contains(needle) {
                    self.found.push(format!("{needle} inside {name}!{{..}}"));
                }
            }
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = &*node.func
            && let Some(last) = path.path.segments.last()
            && METHOD_ROUTER_CONSTRUCTORS.contains(&last.ident.to_string().as_str())
            && !node.args.is_empty()
        {
            self.found.push(format!("{}(..) method router", last.ident));
        }
        visit::visit_expr_call(self, node);
    }
}

fn scan_source(text: &str) -> Vec<String> {
    let file = syn::parse_file(text).expect("source parses");
    let mut scan = Scan { found: Vec::new() };
    scan.visit_file(&file);
    scan.found
}

fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            for hit in scan_source(&text) {
                found.push(format!("{}: {hit}", path.display()));
            }
        }
    }
}

#[test]
fn no_route_is_registered_outside_the_openapi_router() {
    let mut found = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    assert!(
        found.is_empty(),
        "routes registered outside routes! need a policy row and an OpenAPI entry: {found:#?}"
    );
}

#[test]
fn the_scan_catches_planted_registrations() {
    let planted = [
        r#"fn r() -> Router { Router::new().route("/x", get(h)) }"#,
        r#"fn r() -> Router { base.route_service("/x", svc) }"#,
        r#"fn r() -> Router { base.nest("/x", inner) }"#,
        r#"fn r() -> Router { base.nest_service("/x", inner) }"#,
        r#"fn r() -> Router { base.fallback(h) }"#,
        r#"fn r() -> Router { base.merge(other_router) }"#,
        r#"fn r() -> Router { base.merge(build_elsewhere()) }"#,
        r#"fn r() -> Router { base.route("/openapi.json", get(h)).route("/y", post(h)) }"#,
        r#"fn m() -> MethodRouter { post(h) }"#,
        // Production code after a test module is still production code.
        r#"#[cfg(test)] mod tests { fn t() { Router::new().route("/ok", get(h)); } }
           fn r() -> Router { base.route("/y", delete(h)) }"#,
        r#"impl A { fn r(&self) -> Router { base.route("/z", x) } }"#,
        // Registrations inside macro bodies, which syn does not parse.
        r#"fn r() { build!(Router::new().route("/x", get(h))); }"#,
        r#"fn r() { wrap! { base.nest_service("/x", svc) } }"#,
        r#"fn r() { m!(a.merge(other)); }"#,
    ];
    for source in planted {
        assert!(!scan_source(source).is_empty(), "not caught: {source}");
    }
}

#[test]
fn the_scan_allows_the_sanctioned_forms() {
    let allowed = [
        r#"fn r() -> Router { base.route("/openapi.json", get(h)).layer(l) }"#,
        r#"fn r() -> O { base.routes(routes!(a)).merge(uploads).merge(workflows::router()) }"#,
        r#"#[cfg(test)] mod tests { fn t() { Router::new().route("/t", get(h)); } }"#,
        r#"#[cfg(test)] fn t() { Router::new().nest("/t", x); }"#,
        r#"fn r() { let _ = map.get(1); }"#,
        r#"fn r() { let _ = vec![1, 2]; println!("{}", 1); }"#,
        r#"fn r() -> O { base.routes(routes!(a, b)) }"#,
    ];
    for source in allowed {
        assert_eq!(scan_source(source), Vec::<String>::new(), "{source}");
    }
}
