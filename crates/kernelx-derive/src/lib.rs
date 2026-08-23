use proc_macro::TokenStream;

mod export;
mod user_struct;

/// Emits a `.kmodule.exports` record for the annotated function so that loaded
/// kernel modules can resolve it by name.
///
/// The exported name defaults to the function name; pass a string literal to
/// override it:
///
/// ```ignore
/// #[kmodule_export]
/// #[unsafe(no_mangle)]
/// pub extern "C" fn devfs_register(..) -> isize { .. }
///
/// #[kmodule_export("devfs_register_v2")]
/// #[unsafe(no_mangle)]
/// pub extern "C" fn devfs_register_new(..) -> isize { .. }
/// ```
///
/// Use the declarative counterpart (`kmodule_export!("name", path)`) when the
/// exported function is defined elsewhere and cannot be annotated at its
/// definition site.
#[proc_macro_attribute]
pub fn kmodule_export(attr: TokenStream, item: TokenStream) -> TokenStream {
    match export::expand(attr, item.clone()) {
        Ok(output) => output,
        Err(message) => {
            let mut output = item;
            output.extend(compile_error(&message));
            output
        }
    }
}

#[proc_macro_derive(UserStruct)]
pub fn derive_user_struct(input: TokenStream) -> TokenStream {
    match user_struct::expand(input) {
        Ok(output) => output,
        Err(message) => compile_error(&message),
    }
}

fn compile_error(message: &str) -> TokenStream {
    format!("compile_error!({message:?});").parse().unwrap()
}
