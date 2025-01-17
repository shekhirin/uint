#![doc = include_str!("../Readme.md")]
#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};
use std::{fmt::Write, str::FromStr};

/// Construct a `Uint<{bits}>` literal from `limbs`.
fn construct(bits: usize, limbs: &[u64]) -> TokenStream {
    let mut limbs_str = String::new();
    for limb in limbs {
        write!(&mut limbs_str, "{limb}u64,").unwrap();
    }
    let limbs_str = limbs_str.trim_end_matches(',');
    let limbs = (bits + 63) / 64;

    let source = format!(
        "::ruint::Uint::<{}, {}>::from_limbs([{}])",
        bits, limbs, limbs_str
    );
    TokenStream::from_str(&source).unwrap()
}

/// Construct a compiler error message.
// FEATURE: (BLOCKED) Replace with Diagnostic API when stable.
// See <https://doc.rust-lang.org/stable/proc_macro/struct.Diagnostic.html>
fn error(span: Span, message: &str) -> TokenStream {
    // See: https://docs.rs/syn/1.0.70/src/syn/error.rs.html#243
    TokenStream::from_iter(vec![
        TokenTree::Ident(Ident::new("compile_error", span)),
        TokenTree::Punct({
            let mut punct = Punct::new('!', Spacing::Alone);
            punct.set_span(span);
            punct
        }),
        TokenTree::Group({
            let mut group = Group::new(Delimiter::Brace, {
                TokenStream::from_iter(vec![TokenTree::Literal({
                    let mut string = Literal::string(message);
                    string.set_span(span);
                    string
                })])
            });
            group.set_span(span);
            group
        }),
    ])
}

/// Parse a value literal and bits suffix into a Uint literal.
fn parse(value: &str, bits: &str) -> Result<(usize, Vec<u64>), String> {
    // Parse bit length
    let bits = bits
        .parse::<usize>()
        .map_err(|e| format!("Error in suffix: {e}"))?;
    let num_limbs = (bits + 63) / 64;
    let mask = if bits == 0 {
        0
    } else {
        let bits = bits % 64;
        if bits == 0 {
            u64::MAX
        } else {
            (1 << bits) - 1
        }
    };

    // Parse base
    let (base, digits) = if value.len() >= 2 {
        let (prefix, remainder) = value.split_at(2);
        match prefix {
            "0x" => (16_u8, remainder),
            "0o" => (8, remainder),
            "0b" => (2, remainder),
            _ => (10, value),
        }
    } else {
        (10, value)
    };

    // Parse digits in base
    let mut limbs = vec![0_u64];
    for c in digits.chars() {
        // Read next digit
        let digit = match c {
            '0'..='9' => c as u64 - '0' as u64,
            'a'..='f' => c as u64 - 'a' as u64 + 10,
            'A'..='F' => c as u64 - 'A' as u64 + 10,
            '_' => continue,
            _ => return Err(format!("Invalid character '{c}'")),
        };
        #[allow(clippy::cast_lossless)]
        if digit > base as u64 {
            return Err(format!(
                "Invalid digit {c} in base {base} (did you forget the `0x` prefix?)"
            ));
        }

        // Multiply result by base and add digit
        let mut carry = digit;
        #[allow(clippy::cast_lossless)]
        #[allow(clippy::cast_possible_truncation)]
        for limb in &mut limbs {
            let product = (*limb as u128) * (base as u128) + (carry as u128);
            *limb = product as u64;
            carry = (product >> 64) as u64;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }

    // Remove trailing zeros, pad with zeros
    while limbs.len() > num_limbs && limbs.last() == Some(&0) {
        limbs.pop();
    }
    while limbs.len() < num_limbs {
        limbs.push(0);
    }

    // Check value range
    if limbs.len() > num_limbs || limbs.last().copied().unwrap_or(0) > mask {
        let value = value.trim_end_matches('_');
        return Err(format!("Value too large for Uint<{bits}>: {value}"));
    }

    Ok((bits, limbs))
}

/// Transforms a [`Literal`] and returns the substitute [`TokenTree`]
fn transform_literal(literal: Literal) -> TokenTree {
    let source = literal.to_string();
    if let Some((value, bits)) = source.split_once('U') {
        let tokens = parse(value, bits).map_or_else(
            |e| error(literal.span(), &e),
            |(bits, limbs)| construct(bits, &limbs),
        );

        return TokenTree::Group(Group::new(Delimiter::None, tokens));
    }
    TokenTree::Literal(literal)
}

/// Recurse down tree and transform all literals.
fn transform_tree(tree: TokenTree) -> TokenTree {
    match tree {
        TokenTree::Group(group) => {
            let delimiter = group.delimiter();
            let span = group.span();
            let stream = transform_stream(group.stream());
            let mut transformed = Group::new(delimiter, stream);
            transformed.set_span(span);
            TokenTree::Group(transformed)
        }
        TokenTree::Literal(a) => {
            let span = a.span();
            let mut subs = transform_literal(a);
            subs.set_span(span);
            subs
        }
        tree => tree,
    }
}

/// Iterate over a [`TokenStream`] and transform all [`TokenTree`]s.
fn transform_stream(stream: TokenStream) -> TokenStream {
    stream.into_iter().map(transform_tree).collect()
}

// Repeat the crate doc
#[doc = include_str!("../Readme.md")]
#[proc_macro]
pub fn uint(stream: TokenStream) -> TokenStream {
    transform_stream(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_size() {
        assert_eq!(parse("0", "0"), Ok((0, vec![])));
        assert_eq!(parse("00000", "0"), Ok((0, vec![])));
        assert_eq!(parse("0x00", "0"), Ok((0, vec![])));
        assert_eq!(parse("0b0000", "0"), Ok((0, vec![])));
        assert_eq!(parse("0b0000000", "0"), Ok((0, vec![])));
    }

    #[test]
    fn test_bases() {
        assert_eq!(parse("10", "8"), Ok((8, vec![10])));
        assert_eq!(parse("0x10", "8"), Ok((8, vec![16])));
        assert_eq!(parse("0b10", "8"), Ok((8, vec![2])));
        assert_eq!(parse("0o10", "8"), Ok((8, vec![8])));
    }

    #[test]
    #[allow(clippy::unreadable_literal)]
    fn test_overflow_during_parsing() {
        assert_eq!(parse("258664426012969093929703085429980814127835149614277183275038967946009968870203535512256352201271898244626862047232", "384"), Ok((384, vec![0, 15125697203588300800, 6414901478162127871, 13296924585243691235, 13584922160258634318, 121098312706494698])));
        assert_eq!(parse("2135987035920910082395021706169552114602704522356652769947041607822219725780640550022962086936576", "384"), Ok((384, vec![0, 0, 0, 0, 0, 1])));
    }
}
