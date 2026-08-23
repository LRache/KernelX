use proc_macro::{Ident, TokenStream, TokenTree};

/// Appends a `.kmodule.exports` record to the annotated function.
pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream, String> {
    let ident = fn_name(item.clone())
        .ok_or_else(|| "kmodule_export can only be applied to functions".to_string())?;
    let name = export_name(attr)?.unwrap_or_else(|| {
        let ident = ident.to_string();
        format!("{:?}", ident.strip_prefix("r#").unwrap_or(&ident))
    });

    let mut output = item;
    output.extend(
        format!(
            "const _: () = {{
                #[used]
                #[unsafe(link_section = \".kmodule.exports\")]
                static EXPORT: crate::kmodule::exports::KModuleExport =
                    crate::kmodule::exports::KModuleExport::new({name}, {ident} as *const ());
            }};"
        )
        .parse::<TokenStream>()
        .unwrap(),
    );
    Ok(output)
}

/// Returns the string literal passed to the attribute, verbatim, or `None` when
/// the attribute takes no argument.
fn export_name(attr: TokenStream) -> Result<Option<String>, String> {
    let mut tokens = attr.into_iter();
    let Some(token) = tokens.next() else {
        return Ok(None);
    };
    if tokens.next().is_some() {
        return Err("kmodule_export takes at most one string literal".into());
    }

    match &token {
        TokenTree::Literal(literal) if is_str_literal(literal.to_string().as_str()) => {
            Ok(Some(literal.to_string()))
        }
        _ => Err("kmodule_export expects a string literal export name".into()),
    }
}

fn is_str_literal(literal: &str) -> bool {
    literal.starts_with('"') || literal.starts_with("r\"") || literal.starts_with("r#")
}

fn fn_name(input: TokenStream) -> Option<Ident> {
    let mut expect_name = false;
    for token in input {
        match token {
            TokenTree::Ident(ident) if ident.to_string() == "fn" => expect_name = true,
            TokenTree::Ident(ident) if expect_name => return Some(ident),
            _ => {}
        }
    }
    None
}
