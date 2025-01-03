use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields};

#[proc_macro_derive(Sendable)]
pub fn init_messages_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let enum_name = &input.ident;

    let data = match &input.data {
        Data::Enum(data) => data,
        _ => panic!("#[derive(InitMessages)] can only be used on enums"),
    };

    let mut variant_names = Vec::new();
    let mut variant_consts = Vec::new();
    let mut variant_idents = Vec::new();
    let mut match_arms_to_str = Vec::new();

    for variant in &data.variants {
        let variant_name = &variant.ident;
        variant_idents.push(variant.ident.clone());
        let variant_const = format_ident!("{}Const", variant_name.to_string());
        match &variant.fields {
            Fields::Unit => {
                variant_names.push(quote!(#enum_name::#variant_name));
                variant_consts.push(quote! {
                    #[allow(non_upper_case_globals)]
                    pub const #variant_const: &'static str = stringify!(#variant_name);
                });
                match_arms_to_str
                    .push(quote!(#enum_name::#variant_name => stringify!(#variant_name)));
            }
            Fields::Unnamed(fields) => {
                let field_idents: Vec<_> = (0..fields.unnamed.len())
                    .map(|i| format_ident!("_var{}", i))
                    .collect();
                variant_names.push(quote!(#enum_name::#variant_name(#(#field_idents),*)));
                variant_consts.push(quote! {
                    #[allow(non_upper_case_globals)]
                    pub const #variant_const: &'static str = stringify!(#variant_name);
                });
                match_arms_to_str
                    .push(quote!(#enum_name::#variant_name(..) => stringify!(#variant_name)));
            }
            Fields::Named(_) => panic!("#[derive(InitMessages)] does not support named fields"),
        }
    }

    let nb_senders = variant_names.len();

    let output = quote! {
        impl #enum_name {
            #(#variant_consts)*
        }

        impl crate::system::message_interface::SendableMessage for #enum_name {

            const NB_SENDERS: usize = #nb_senders;

            fn str_to_id(the_s: &str) -> usize {
                const MESSAGE_STRING_ARR: [&'static str; #nb_senders] = [
                    #(stringify!(#variant_idents)),*
                ];
                MESSAGE_STRING_ARR
                    .iter()
                    .position(|s| s == &the_s)
                    .unwrap()
            }

            fn get_id(&self) -> usize {
                Self::str_to_id(self.to_str())
            }

            fn to_str(&self) -> &'static str {
                match self {
                    #(#match_arms_to_str,)*
                }
            }

            fn is_close(&self) -> bool {
                matches!(self, #enum_name::Close)
            }

            fn close() -> Self {
                #enum_name::Close
            }
        }
    };

    output.into()
}
