use proc_macro::{Delimiter, Group, Ident, TokenStream, TokenTree};

/// Implements `UserStruct` for the derived `#[repr(C)]` struct or union.
pub(crate) fn expand(input: TokenStream) -> Result<TokenStream, String> {
    if !has_repr_c(input.clone()) {
        return Err("UserStruct can only be derived for types marked with #[repr(C)]".into());
    }

    let name =
        item_name(input).ok_or_else(|| "UserStruct can only be derived for structs or unions".to_string())?;
    Ok(format!("impl crate::kernel::syscall::UserStruct for {name} {{}}")
        .parse()
        .unwrap())
}

fn has_repr_c(input: TokenStream) -> bool {
    let mut tokens = input.into_iter();
    while let Some(token) = tokens.next() {
        if let TokenTree::Punct(punct) = &token
            && punct.as_char() == '#'
            && let Some(TokenTree::Group(group)) = tokens.next()
            && group.delimiter() == Delimiter::Bracket
            && is_repr_c(group)
        {
            return true;
        }
    }
    false
}

fn is_repr_c(group: Group) -> bool {
    let mut tokens = group.stream().into_iter();
    matches!(tokens.next(), Some(TokenTree::Ident(ident)) if ident.to_string() == "repr")
        && matches!(
            tokens.next(),
            Some(TokenTree::Group(args))
                if args.delimiter() == Delimiter::Parenthesis && contains_c(args.stream())
        )
}

fn contains_c(stream: TokenStream) -> bool {
    stream
        .into_iter()
        .any(|token| matches!(token, TokenTree::Ident(ident) if ident.to_string() == "C"))
}

fn item_name(input: TokenStream) -> Option<Ident> {
    let mut expect_name = false;
    for token in input {
        match token {
            TokenTree::Ident(ident) if ident.to_string() == "struct" || ident.to_string() == "union" => {
                expect_name = true;
            }
            TokenTree::Ident(ident) if expect_name => return Some(ident),
            _ => {}
        }
    }
    None
}
