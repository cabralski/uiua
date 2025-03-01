use std::{error::Error, fmt, iter::once, path::Path};

use crate::{
    ast::*,
    function::{FunctionId, Signature},
    lex::{AsciiToken::*, Token::*, *},
    Diagnostic, DiagnosticKind, Ident, Primitive,
};

#[derive(Debug, Clone)]
pub enum ParseError {
    Lex(LexError),
    Expected(Vec<Expectation>, Option<Box<Sp<Token>>>),
    InvalidNumber(String),
    Unexpected(Token),
    InvalidArgCount(String),
    InvalidOutCount(String),
    AmpersandBindingName,
    FunctionNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    Term,
    ArgOutCount,
    Simple(AsciiToken),
}

impl From<AsciiToken> for Expectation {
    fn from(simple: AsciiToken) -> Self {
        Expectation::Simple(simple)
    }
}

impl fmt::Display for Expectation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expectation::Term => write!(f, "term"),
            Expectation::ArgOutCount => write!(f, "argument/output count"),
            Expectation::Simple(s) => write!(f, "`{s}`"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Lex(e) => write!(f, "{e}"),
            ParseError::Expected(exps, found) => {
                write!(f, "Expected ")?;
                if exps.len() == 2 {
                    write!(f, "{} or {}", exps[0], exps[1])?;
                } else {
                    for (i, exp) in exps.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{exp}")?;
                    }
                }
                if let Some(found) = found {
                    if let Token::Simple(ascii) = found.value {
                        if exps.iter().any(|exp| exp == &Expectation::Simple(ascii)) {
                            return Ok(());
                        }
                    }
                    let found = found.span.as_str();
                    if found == "\n" {
                        write!(f, ", found newline")?;
                    } else {
                        write!(f, ", found `{found}`")?;
                    }
                }
                Ok(())
            }
            ParseError::InvalidNumber(s) => write!(f, "Invalid number `{s}`"),
            ParseError::Unexpected(_) => write!(f, "Unexpected token"),
            ParseError::InvalidArgCount(n) => write!(f, "Invalid argument count `{n}`"),
            ParseError::InvalidOutCount(n) => write!(f, "Invalid output count `{n}`"),
            ParseError::AmpersandBindingName => write!(f, "Binding names may not contain `&`"),
            ParseError::FunctionNotAllowed => write!(
                f,
                "Inline functions are only allowed in modifiers \
                or as the only item in a binding"
            ),
        }
    }
}

impl Error for ParseError {}

/// Parse Uiua code into an AST
pub fn parse(
    input: &str,
    path: Option<&Path>,
) -> (Vec<Item>, Vec<Sp<ParseError>>, Vec<Diagnostic>) {
    let (tokens, lex_errors) = lex(input, path);
    let errors = lex_errors
        .into_iter()
        .map(|e| e.map(ParseError::Lex))
        .collect();
    let mut parser = Parser {
        tokens,
        index: 0,
        errors,
        diagnostics: Vec::new(),
    };
    let items = parser.items(true);
    if parser.errors.is_empty() && parser.index < parser.tokens.len() {
        parser.errors.push(
            parser
                .tokens
                .remove(parser.index)
                .map(ParseError::Unexpected),
        );
    }
    (items, parser.errors, parser.diagnostics)
}

struct Parser {
    tokens: Vec<Sp<crate::lex::Token>>,
    index: usize,
    errors: Vec<Sp<ParseError>>,
    diagnostics: Vec<Diagnostic>,
}

type FunctionContents = (Option<Sp<Signature>>, Vec<Vec<Sp<Word>>>, Option<CodeSpan>);

impl Parser {
    fn next_token_map<'a, T: 'a>(
        &'a mut self,
        f: impl FnOnce(&'a Token) -> Option<T>,
    ) -> Option<Sp<T>> {
        let token = self.tokens.get(self.index)?;
        if let Some(value) = f(&token.value) {
            self.index += 1;
            Some(token.span.clone().sp(value))
        } else {
            None
        }
    }
    fn try_exact(&mut self, token: impl Into<Token>) -> Option<CodeSpan> {
        let token = token.into();
        self.next_token_map(|t| (t == &token).then_some(()))
            .map(|t| t.span)
    }
    fn prev_span(&self) -> CodeSpan {
        if let Some(token) = self.tokens.get(self.index.saturating_sub(1)) {
            token.span.clone()
        } else {
            self.tokens.last().unwrap().span.clone()
        }
    }
    fn expected<I: Into<Expectation>>(
        &self,
        expectations: impl IntoIterator<Item = I>,
    ) -> Sp<ParseError> {
        self.prev_span().sp(ParseError::Expected(
            expectations.into_iter().map(Into::into).collect(),
            self.tokens
                .get(self.index.saturating_sub(1))
                .cloned()
                .map(Box::new),
        ))
    }
    #[allow(unused)]
    fn expected_continue<I: Into<Expectation>>(
        &mut self,
        expectations: impl IntoIterator<Item = I>,
    ) {
        let err = self.prev_span().sp(ParseError::Expected(
            expectations.into_iter().map(Into::into).collect(),
            None,
        ));
        self.errors.push(err);
    }
    fn items(&mut self, parse_scopes: bool) -> Vec<Item> {
        let mut items = Vec::new();
        loop {
            match self.try_item(parse_scopes) {
                Some(item) => items.push(item),
                None => {
                    if self.try_exact(Newline).is_none() {
                        break;
                    }
                    let mut newline_span: Option<CodeSpan> = None;
                    while let Some(span) = self.try_exact(Newline) {
                        newline_span = Some(if let Some(prev) = newline_span.take() {
                            prev.merge(span)
                        } else {
                            span
                        });
                    }
                    items.extend(newline_span.map(Item::ExtraNewlines));
                }
            }
        }
        items
    }
    fn try_item(&mut self, parse_scopes: bool) -> Option<Item> {
        self.try_spaces();
        Some(if let Some(binding) = self.try_binding() {
            Item::Binding(binding)
        } else if let Some(words) = self.try_words() {
            Item::Words(words)
        } else if parse_scopes && self.try_exact(TripleMinus).is_some() {
            let items = self.items(false);
            if self.try_exact(TripleMinus).is_none() {
                self.errors.push(self.expected([TripleMinus]));
            }
            Item::TestScope(items)
        } else {
            return None;
        })
    }
    fn comment(&mut self) -> Option<Sp<String>> {
        let span = self.try_exact(Token::Comment)?;
        let s = span.as_str();
        let s = s.strip_prefix('#').unwrap_or(s).into();
        Some(span.sp(s))
    }
    fn try_binding(&mut self) -> Option<Binding> {
        let start = self.index;
        Some(if let Some(name) = self.try_ident() {
            // Check for invalid binding names
            if name.value.contains('&') {
                self.errors
                    .push(self.prev_span().sp(ParseError::AmpersandBindingName));
            }
            // Left arrow
            let mut arrow_span = self.try_spaces().map(|w| w.span);
            if let Some(span) = self.try_exact(Equal).or_else(|| self.try_exact(LeftArrow)) {
                arrow_span = Some(if let Some(arrow_span) = arrow_span {
                    arrow_span.merge(span)
                } else {
                    span
                });
            } else {
                self.index = start;
                return None;
            }
            let mut arrow_span = arrow_span.unwrap();
            if let Some(span) = self.try_spaces().map(|w| w.span) {
                arrow_span = arrow_span.merge(span);
            }
            // Signature
            let signature = self.try_signature(Bar);
            // Words
            let words = self.try_words().unwrap_or_default();
            match words.as_slice() {
                [Sp {
                    value: Word::Func(func),
                    ..
                }] => {
                    for line in &func.lines {
                        self.validate_words(line, false);
                    }
                }
                words => self.validate_words(words, false),
            }
            // Check for uncapitalized binding names
            if name.value.trim_end_matches('!').chars().count() >= 3
                && name.value.chars().next().unwrap().is_ascii_lowercase()
            {
                let captialized: String = name
                    .value
                    .chars()
                    .next()
                    .map(|c| c.to_ascii_uppercase())
                    .into_iter()
                    .chain(name.value.chars().skip(1))
                    .collect();
                self.diagnostics.push(Diagnostic::new(
                    format!(
                        "Binding names with 3 or more characters should be TitleCase \
                        to avoid collisions with future builtin functions.\n\
                        Try `{}` instead of `{}`",
                        captialized, name.value
                    ),
                    name.span.clone(),
                    DiagnosticKind::Advice,
                ));
            }
            Binding {
                name,
                arrow_span,
                words,
                signature,
            }
        } else {
            return None;
        })
    }
    fn try_ident(&mut self) -> Option<Sp<Ident>> {
        let span = self.try_exact(Token::Ident)?;
        let s: Ident = span.as_str().into();
        Some(span.sp(s))
    }
    fn try_modifier_ident(&mut self) -> Option<Sp<Ident>> {
        let start = self.index;
        let ident = self.try_ident()?;
        if ident_modifier_args(&ident.value) == 0 {
            self.index = start;
            return None;
        }
        Some(ident)
    }
    fn try_signature(&mut self, initial_token: AsciiToken) -> Option<Sp<Signature>> {
        let start = self.try_exact(initial_token)?;
        self.try_spaces();
        let (args, outs) = self.sig_inner();
        let end = self.prev_span();
        self.try_spaces();
        let span = start.merge(end);
        Some(span.sp(Signature::new(args, outs)))
    }
    fn sig_inner(&mut self) -> (usize, usize) {
        if let Some(sn) = self.try_num() {
            if let Some((a, o)) = sn.value.0.split_once('.') {
                let a = match a.parse() {
                    Ok(a) => a,
                    Err(_) => {
                        self.errors
                            .push(self.prev_span().sp(ParseError::InvalidArgCount(a.into())));
                        1
                    }
                };
                let o = match o.parse() {
                    Ok(o) => o,
                    Err(_) => {
                        self.errors
                            .push(self.prev_span().sp(ParseError::InvalidOutCount(o.into())));
                        1
                    }
                };
                (a, o)
            } else {
                let a = match sn.value.0.parse() {
                    Ok(a) => a,
                    Err(_) => {
                        self.errors
                            .push(self.prev_span().sp(ParseError::InvalidArgCount(sn.value.0)));
                        1
                    }
                };
                (a, 1)
            }
        } else {
            self.errors.push(self.expected([Expectation::ArgOutCount]));
            (1usize, 1usize)
        }
    }
    fn try_words(&mut self) -> Option<Vec<Sp<Word>>> {
        let mut words: Vec<Sp<Word>> = Vec::new();
        while let Some(word) = self.try_word() {
            if let Some(prev) = words.last() {
                // Style diagnostics
                use Primitive::*;
                let span = || prev.span.clone().merge(word.span.clone());
                if let (Word::Primitive(a), Word::Primitive(b)) = (&prev.value, &word.value) {
                    match (a, b) {
                        (Flip, Over) => self.diagnostics.push(Diagnostic::new(
                            format!("Prefer `{Dip}{Dup}` over `{Flip}{Over}` for clarity"),
                            span(),
                            DiagnosticKind::Style,
                        )),
                        // Not comparisons
                        (Not, prim) => {
                            for (a, b) in [(Eq, Ne), (Lt, Ge), (Gt, Le)] {
                                if *prim == a {
                                    self.diagnostics.push(Diagnostic::new(
                                        format!("Prefer `{b}` over `{Not}{prim}` for clarity"),
                                        span(),
                                        DiagnosticKind::Style,
                                    ));
                                } else if *prim == b {
                                    self.diagnostics.push(Diagnostic::new(
                                        format!("Prefer `{a}` over `{Not}{prim}` for clarity"),
                                        span(),
                                        DiagnosticKind::Style,
                                    ));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            words.push(word);
        }
        if words.is_empty() {
            None
        } else {
            Some(words)
        }
    }
    fn multiline_words(&mut self) -> Vec<Vec<Sp<Word>>> {
        let mut lines = Vec::new();
        while self.try_exact(Newline).is_some() || self.try_spaces().is_some() {}
        while let Some(words) = self.try_words() {
            lines.push(words);
            let mut newlines = 0;
            while self.try_exact(Newline).is_some() {
                newlines += 1;
                self.try_spaces();
            }
            if newlines > 1 {
                lines.push(Vec::new());
            }
        }
        if lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        lines
    }
    fn try_word(&mut self) -> Option<Sp<Word>> {
        self.comment()
            .map(|c| c.map(Word::Comment))
            .or_else(|| self.try_strand())
            .or_else(|| self.try_placeholder())
    }
    fn try_strand(&mut self) -> Option<Sp<Word>> {
        let word = self.try_modified()?;
        if let Word::Spaces = word.value {
            return Some(word);
        }
        // Collect items
        let mut items = Vec::new();
        let mut singleton = false;
        while self.try_exact(Underscore).is_some() {
            let item = match self.try_modified() {
                Some(mut item) => {
                    if let Word::Spaces = item.value {
                        if items.is_empty() {
                            singleton = true;
                            break;
                        }
                        self.errors.push(self.expected([Expectation::Term]));
                        item = match self.try_modified() {
                            Some(item) => item,
                            None => {
                                self.errors.push(self.expected([Expectation::Term]));
                                break;
                            }
                        };
                    }
                    item
                }
                None if items.is_empty() => {
                    singleton = true;
                    break;
                }
                None => {
                    self.errors.push(self.expected([Expectation::Term]));
                    break;
                }
            };
            items.push(item);
        }
        // If there is only one item and no underscores, return it
        if items.is_empty() && !singleton {
            return Some(word);
        }
        // Insert the first word that was parsed
        items.insert(0, word);
        let span = items[0]
            .span
            .clone()
            .merge(items.last().unwrap().span.clone());
        Some(span.sp(Word::Strand(items)))
    }
    fn try_modified(&mut self) -> Option<Sp<Word>> {
        let (modifier, mod_span) = if let Some(prim) = Primitive::all()
            .filter(|prim| prim.is_modifier())
            .find_map(|prim| {
                self.try_exact(prim)
                    .or_else(|| prim.ascii().and_then(|simple| self.try_exact(simple)))
                    .map(|span| span.sp(prim))
            }) {
            (Modifier::Primitive(prim.value), prim.span)
        } else if let Some(ident) = self.try_modifier_ident() {
            (Modifier::Ident(ident.value), ident.span)
        } else {
            return self.try_term();
        };
        let mut args = Vec::new();
        self.try_spaces();
        let mut arg_count = 0;
        for _ in 0..modifier.args() {
            args.extend(self.try_spaces());
            if let Some(arg) = self
                .try_func()
                .or_else(|| self.try_strand())
                .or_else(|| self.try_placeholder())
            {
                args.push(arg);
                arg_count += 1;
            } else {
                break;
            }
        }
        if arg_count != modifier.args() {
            self.errors.push(self.expected([Expectation::Term]));
        }

        // Style diagnostics
        match modifier {
            Modifier::Primitive(Primitive::Bind) => {
                for arg in &args {
                    if let Word::Modified(m) = &arg.value {
                        if let Modifier::Primitive(Primitive::Bind) = m.modifier.value {
                            let span = mod_span.clone().merge(m.modifier.span.clone());
                            self.diagnostics.push(Diagnostic::new(
                                format!("Do not chain `bind {}`", Primitive::Bind),
                                span,
                                DiagnosticKind::Style,
                            ));
                        } else if m.modifier.value.args() > 1 {
                            let span = mod_span.clone().merge(m.modifier.span.clone());
                            self.diagnostics.push(Diagnostic::new(
                                format!(
                                    "Do not use non-monadic modifiers inside `bind {}`",
                                    Primitive::Bind
                                ),
                                span,
                                DiagnosticKind::Style,
                            ));
                        }
                    }
                }
            }
            Modifier::Primitive(Primitive::Oust) => {
                for arg in &args {
                    if let Word::Modified(m) = &arg.value {
                        match &m.modifier.value {
                            Modifier::Primitive(Primitive::Dip) => {
                                let span = mod_span.clone().merge(m.modifier.span.clone());
                                self.diagnostics.push(Diagnostic::new(
                                    format!(
                                        "`{oust}{dip}` is either unclear or not what you want. \
                                    If you want the same behavior, prefer `{dip}{gap}` \
                                    for clarity. If you mean to call a function on the \
                                    first and third arguments, use `{oust}f`.",
                                        oust = Primitive::Oust,
                                        dip = Primitive::Dip,
                                        gap = Primitive::Gap,
                                    ),
                                    span,
                                    DiagnosticKind::Style,
                                ));
                            }
                            Modifier::Primitive(Primitive::Gap) => {
                                let span = mod_span.clone().merge(m.modifier.span.clone());
                                self.diagnostics.push(Diagnostic::new(
                                    format!(
                                        "`{oust}{gap}` is either unclear or not what you want. \
                                    If you want the same behavior, prefer `{gap}{gap}` \
                                    for clarity. If you mean to call a function on the \
                                    first and fourth arguments, use `{oust}{oust}f`.",
                                        oust = Primitive::Oust,
                                        gap = Primitive::Gap,
                                    ),
                                    span,
                                    DiagnosticKind::Style,
                                ));
                            }
                            _ => (),
                        }
                    }
                }
            }
            _ => {}
        }

        Some(if args.is_empty() {
            mod_span.sp(match modifier {
                Modifier::Primitive(prim) => Word::Primitive(prim),
                Modifier::Ident(ident) => Word::Ident(ident),
            })
        } else {
            for arg in &mut args {
                if let Word::Func(func) = &arg.value {
                    if func.lines.is_empty() && func.signature.is_none() {
                        arg.value = Word::Primitive(Primitive::Identity);
                    }
                }
            }
            let span = mod_span.clone().merge(args.last().unwrap().span.clone());
            span.sp(Word::Modified(Box::new(Modified {
                modifier: mod_span.sp(modifier),
                operands: args,
            })))
        })
    }
    fn try_placeholder(&mut self) -> Option<Sp<Word>> {
        let sig = self.try_signature(Caret)?;
        Some(sig.map(Word::Placeholder))
    }
    fn try_term(&mut self) -> Option<Sp<Word>> {
        Some(if let Some(prim) = self.try_prim() {
            if prim.value.is_ocean() {
                let mut ocean_parts = Vec::new();
                while let Some(part) = self.try_ocean() {
                    ocean_parts.push(part);
                }
                if ocean_parts.is_empty() {
                    prim.map(Word::Primitive)
                } else {
                    let span = prim
                        .span
                        .clone()
                        .merge(ocean_parts.last().unwrap().span.clone());
                    span.sp(Word::Ocean(once(prim).chain(ocean_parts).collect()))
                }
            } else {
                prim.map(Word::Primitive)
            }
        } else if let Some(ident) = self.try_ident() {
            ident.map(Word::Ident)
        } else if let Some(sn) = self.try_num() {
            sn.map(|(s, n)| Word::Number(s, n))
        } else if let Some(c) = self.next_token_map(Token::as_char) {
            c.map(Into::into).map(Word::Char)
        } else if let Some(s) = self.next_token_map(Token::as_string) {
            s.map(Into::into).map(Word::String)
        } else if let Some(frags) = self.next_token_map(Token::as_format_string) {
            frags.map(Word::FormatString)
        } else if let Some(line) = self.next_token_map(Token::as_multiline_string) {
            let start = line.span.clone();
            let mut end = start.clone();
            let mut lines = vec![line];
            while let Some(line) = self.next_token_map(Token::as_multiline_string) {
                end = line.span.clone();
                lines.push(line);
            }
            let span = start.merge(end);
            span.sp(Word::MultilineString(lines))
        } else if let Some(start) = self.try_exact(OpenBracket) {
            let items = self.multiline_words();
            let end = self.expect_close(CloseBracket);
            let span = start.merge(end);
            span.sp(Word::Array(Arr {
                lines: items,
                constant: false,
            }))
        } else if let Some(start) = self.try_exact(OpenCurly) {
            let items = self.multiline_words();
            let end = self.expect_close(CloseCurly);
            let span = start.merge(end);
            span.sp(Word::Array(Arr {
                lines: items,
                constant: true,
            }))
        } else if let Some(spaces) = self.try_spaces() {
            spaces
        } else if let Some(mut word) = self.try_func() {
            if let Word::Func(func) = &word.value {
                if func.lines.is_empty() && func.signature.is_none() {
                    word.value = Word::Primitive(Primitive::Identity);
                }
            }
            word
        } else {
            return None;
        })
    }
    fn try_num(&mut self) -> Option<Sp<(String, f64)>> {
        let span = self.try_exact(Token::Number)?;
        let s = span.as_str().to_string();
        let parseable = s.replace(['`', '¯'], "-");
        let n: f64 = match parseable.parse() {
            Ok(n) => n,
            Err(_) => {
                self.errors
                    .push(self.prev_span().sp(ParseError::InvalidNumber(s.clone())));
                0.0
            }
        };
        Some(span.sp((s, n)))
    }
    fn try_prim(&mut self) -> Option<Sp<Primitive>> {
        for prim in Primitive::all() {
            let op_span = self
                .try_exact(prim)
                .or_else(|| prim.ascii().and_then(|simple| self.try_exact(simple)));
            if let Some(span) = op_span {
                return Some(span.sp(prim));
            }
        }
        None
    }
    fn try_ocean(&mut self) -> Option<Sp<Primitive>> {
        for prim in Primitive::all().filter(Primitive::is_ocean) {
            let op_span = self
                .try_exact(prim)
                .or_else(|| prim.ascii().and_then(|simple| self.try_exact(simple)));
            if let Some(span) = op_span {
                return Some(span.sp(prim));
            }
        }
        None
    }
    fn try_func(&mut self) -> Option<Sp<Word>> {
        Some(if let Some(start) = self.try_exact(OpenParen) {
            let first = self.func_contents();
            let mut branches = Vec::new();
            while let Some(start) = self.try_exact(Bar) {
                let (signature, lines, span) = self.func_contents();
                let span = if let Some(span) = span {
                    start.merge(span)
                } else {
                    start
                };
                let id = FunctionId::Anonymous(span.clone());
                branches.push(span.sp(Func {
                    id,
                    signature,
                    lines,
                }))
            }
            let end = self.expect_close(CloseParen);
            let (signature, lines, first_span) = first;
            let outer_span = start.clone().merge(end);
            if branches.is_empty() {
                let id = FunctionId::Anonymous(outer_span.clone());
                outer_span.sp(Word::Func(Func {
                    id,
                    signature,
                    lines,
                }))
            } else {
                let span = first_span.unwrap_or(start);
                let id = FunctionId::Anonymous(span.clone());
                let first = span.sp(Func {
                    id,
                    signature,
                    lines,
                });
                branches.insert(0, first);
                outer_span.sp(Word::Switch(Switch { branches }))
            }
        } else {
            return None;
        })
    }
    fn func_contents(&mut self) -> FunctionContents {
        while self.try_exact(Newline).is_some() || self.try_spaces().is_some() {}
        let signature = self.try_signature(Bar);
        let lines = self.multiline_words();
        let start = signature
            .as_ref()
            .map(|sig| sig.span.clone())
            .or_else(|| lines.iter().flatten().next().map(|word| word.span.clone()));
        let end = lines
            .iter()
            .flatten()
            .last()
            .map(|word| word.span.clone())
            .or_else(|| signature.as_ref().map(|sig| sig.span.clone()));
        let span = start.zip(end).map(|(start, end)| start.merge(end));
        (signature, lines, span)
    }
    fn try_spaces(&mut self) -> Option<Sp<Word>> {
        self.try_exact(Spaces).map(|span| span.sp(Word::Spaces))
    }
    fn expect_close(&mut self, ascii: AsciiToken) -> CodeSpan {
        if let Some(span) = self.try_exact(ascii) {
            span
        } else {
            self.errors
                .push(self.expected([Expectation::Term, Expectation::Simple(ascii)]));
            self.prev_span()
        }
    }
    fn validate_words(&mut self, words: &[Sp<Word>], allow_func: bool) {
        for word in words {
            match &word.value {
                Word::Strand(items) => self.validate_words(items, false),
                Word::Array(arr) => {
                    for line in &arr.lines {
                        self.validate_words(line, false);
                    }
                }
                Word::Func(func) => {
                    if !allow_func {
                        self.errors
                            .push(word.span.clone().sp(ParseError::FunctionNotAllowed));
                    }
                    for line in &func.lines {
                        self.validate_words(line, false);
                    }
                }
                Word::Switch(sw) => {
                    for branch in &sw.branches {
                        for line in &branch.value.lines {
                            self.validate_words(line, false);
                        }
                    }
                }
                Word::Modified(m) => self.validate_words(&m.operands, true),
                _ => {}
            }
        }
    }
}

pub(crate) fn ident_modifier_args(ident: &Ident) -> u8 {
    let mut count: u8 = 0;
    let mut prefix = ident.as_ref();
    while let Some(pre) = prefix.strip_suffix('!') {
        prefix = pre;
        count = count.saturating_add(1);
    }
    count
}

pub(crate) fn count_placeholders(words: &[Sp<Word>]) -> usize {
    let mut count = 0;
    for word in words {
        match &word.value {
            Word::Placeholder(_) => count += 1,
            Word::Strand(items) => count += count_placeholders(items),
            Word::Array(arr) => {
                for line in &arr.lines {
                    count += count_placeholders(line);
                }
            }
            Word::Func(func) => {
                for line in &func.lines {
                    count += count_placeholders(line);
                }
            }
            Word::Modified(m) => count += count_placeholders(&m.operands),
            _ => {}
        }
    }
    count
}
