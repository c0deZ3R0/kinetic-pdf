//! PDF functions, as far as colours need them: the tint transforms that turn a
//! Separation or DeviceN colour into its alternate space.
//!
//! All four types are read: sampled (0), exponential (2), stitching (3) and
//! PostScript calculator (4). Print and CAD exports set most of their linework
//! in spot inks this way -- one civil drawing set draws every line of all its
//! 137 sheets in a "Black" separation with a sampled transform -- so without
//! these a whole file goes to pdfium.

use pdf_content::lopdf::{Document, Object};
use pdf_content::objects::number;

/// A function of `m` inputs to `n` outputs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Function {
    /// Each input's `[low, high]`, which inputs are clipped to.
    domain: Vec<[f32; 2]>,
    /// Each output's `[low, high]`, which outputs are clipped to; for a
    /// function that doesn't say, as many outputs as it makes, unclipped.
    range: Option<Vec<[f32; 2]>>,
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// A table of samples, `size[i]` along input `i`, the first input varying
    /// fastest, `outputs` values each, already decoded into their output
    /// range.
    Sampled { size: Vec<usize>, encode: Vec<[f32; 2]>, outputs: usize, samples: Vec<f32> },
    /// `c0 + x^n * (c1 - c0)`, for one input.
    Exponential { c0: Vec<f32>, c1: Vec<f32>, n: f32 },
    /// One of `functions` for each stretch of the one input, split at
    /// `bounds`, each stretch mapped into its function's domain by `encode`.
    Stitching { functions: Vec<Function>, bounds: Vec<f32>, encode: Vec<[f32; 2]> },
    /// A PostScript calculator program.
    PostScript(Vec<Op>),
}

/// Most values a function's calculations may put on the stack, against a
/// program that loops on `copy`.
const DEEPEST_STACK: usize = 100;

/// Most samples a sampled function may hold, against a table sized to
/// exhaust memory.
const MOST_SAMPLES: usize = 1 << 24;

impl Function {
    /// The function written as `object`: a dictionary, or a stream for types
    /// 0 and 4. `None` if it's malformed or of a type there isn't.
    pub fn read(doc: &Document, object: &Object) -> Option<Function> {
        Self::read_nested(doc, object, 0)
    }

    fn read_nested(doc: &Document, object: &Object, depth: usize) -> Option<Function> {
        if depth > 8 {
            return None;
        }
        let (_, object) = doc.dereference(object).ok()?;
        let (dict, stream) = match object {
            Object::Dictionary(dict) => (dict, None),
            Object::Stream(stream) => (&stream.dict, Some(stream)),
            _ => return None,
        };
        let numbers = |key: &[u8]| dict.get(key).ok().and_then(|o| numbers(doc, o));
        let pairs = |key: &[u8]| numbers(key).map(|n| n.chunks_exact(2).map(|p| [p[0], p[1]]).collect::<Vec<_>>());
        let domain = pairs(b"Domain").filter(|d| !d.is_empty())?;
        let range = pairs(b"Range");
        let kind = match dict.get(b"FunctionType").ok().and_then(|t| number(doc, t))? as i64 {
            0 => {
                let range = range.as_ref()?;
                let size: Vec<usize> = numbers(b"Size")?.iter().map(|&s| s as usize).collect();
                if size.len() != domain.len() || size.contains(&0) {
                    return None;
                }
                let bits = dict.get(b"BitsPerSample").ok().and_then(|b| number(doc, b))? as u32;
                if !matches!(bits, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
                    return None;
                }
                let outputs = range.len();
                let count = size.iter().try_fold(outputs, |n, &s| n.checked_mul(s)).filter(|&n| n <= MOST_SAMPLES)?;
                let encode = pairs(b"Encode").unwrap_or_else(|| size.iter().map(|&s| [0.0, (s - 1) as f32]).collect());
                let decode = pairs(b"Decode").unwrap_or_else(|| range.clone());
                let data = stream?.decompressed_content().ok().or_else(|| Some(stream?.content.clone()))?;
                let most = ((1_u64 << bits) - 1) as f32;
                let samples = (0..count)
                    .map(|i| {
                        let [low, high] = decode.get(i % outputs).copied().unwrap_or([0.0, 1.0]);
                        low + read_bits(&data, i * bits as usize, bits) as f32 * (high - low) / most
                    })
                    .collect();
                Kind::Sampled { size, encode, outputs, samples }
            }
            2 => {
                let c0 = numbers(b"C0").unwrap_or_else(|| vec![0.0]);
                let c1 = numbers(b"C1").unwrap_or_else(|| vec![1.0]);
                let n = dict.get(b"N").ok().and_then(|n| number(doc, n))? as f32;
                if c0.len() != c1.len() {
                    return None;
                }
                Kind::Exponential { c0, c1, n }
            }
            3 => {
                let functions = match doc.dereference(dict.get(b"Functions").ok()?).ok()?.1 {
                    Object::Array(items) => items.iter().map(|f| Self::read_nested(doc, f, depth + 1)).collect::<Option<Vec<_>>>()?,
                    _ => return None,
                };
                let bounds = numbers(b"Bounds").unwrap_or_default();
                let encode = pairs(b"Encode")?;
                if functions.is_empty() || bounds.len() + 1 != functions.len() || encode.len() != functions.len() {
                    return None;
                }
                Kind::Stitching { functions, bounds, encode }
            }
            4 => {
                range.as_ref()?;
                let program = stream?.decompressed_content().ok().or_else(|| Some(stream?.content.clone()))?;
                Kind::PostScript(parse_postscript(&program)?)
            }
            _ => return None,
        };
        Some(Function { domain, range, kind })
    }

    /// How many values the function takes.
    pub fn inputs(&self) -> usize {
        self.domain.len()
    }

    /// The function's outputs for `input`, or `None` if it can't give them.
    pub fn eval(&self, input: &[f32]) -> Option<Vec<f32>> {
        let x: Vec<f32> = self.domain.iter().enumerate().map(|(i, &[low, high])| input.get(i).copied().unwrap_or(low).clamp(low.min(high), high.max(low))).collect();
        let mut out = match &self.kind {
            Kind::Sampled { size, encode, outputs, samples } => sampled(size, encode, *outputs, samples, &self.domain, &x),
            Kind::Exponential { c0, c1, n } => {
                let t = x[0].powf(*n);
                c0.iter().zip(c1).map(|(a, b)| a + t * (b - a)).collect()
            }
            Kind::Stitching { functions, bounds, encode } => {
                let t = x[0];
                let [low, high] = self.domain[0];
                let k = bounds.iter().take_while(|&&b| t >= b).count();
                let from = if k == 0 { low } else { bounds[k - 1] };
                let to = if k == bounds.len() { high } else { bounds[k] };
                functions[k].eval(&[interpolate(t, from, to, encode[k][0], encode[k][1])])?
            }
            Kind::PostScript(program) => {
                let mut stack: Vec<Value> = x.iter().map(|&v| Value::Number(v)).collect();
                run(program, &mut stack)?;
                let outputs = self.range.as_ref().map_or(0, Vec::len);
                let start = stack.len().checked_sub(outputs)?;
                stack[start..].iter().map(|v| v.number()).collect::<Option<Vec<_>>>()?
            }
        };
        if let Some(range) = &self.range {
            for (value, &[low, high]) in out.iter_mut().zip(range) {
                *value = value.clamp(low.min(high), high.max(low));
            }
        }
        Some(out)
    }
}

/// `x` mapped from `[x0, x1]` to `[y0, y1]`.
fn interpolate(x: f32, x0: f32, x1: f32, y0: f32, y1: f32) -> f32 {
    if x1 == x0 {
        y0
    } else {
        y0 + (x - x0) * (y1 - y0) / (x1 - x0)
    }
}

/// `bits` bits of `data` from bit `at`, most significant first; missing bits
/// read as 0.
fn read_bits(data: &[u8], at: usize, bits: u32) -> u32 {
    if bits == 8 {
        return u32::from(data.get(at / 8).copied().unwrap_or(0));
    }
    let mut value = 0_u64;
    for i in 0..bits as usize {
        let bit = at + i;
        let byte = data.get(bit / 8).copied().unwrap_or(0);
        value = value << 1 | u64::from(byte >> (7 - bit % 8) & 1);
    }
    value as u32
}

/// A sampled function at `x`, interpolated linearly between its samples along
/// every input.
fn sampled(size: &[usize], encode: &[[f32; 2]], outputs: usize, samples: &[f32], domain: &[[f32; 2]], x: &[f32]) -> Vec<f32> {
    let inputs = size.len();
    // For each input, the sample below, the one above, and how far between.
    let mut corners = Vec::with_capacity(inputs);
    for i in 0..inputs {
        let [e0, e1] = encode.get(i).copied().unwrap_or([0.0, (size[i] - 1) as f32]);
        let e = interpolate(x[i], domain[i][0], domain[i][1], e0, e1).clamp(0.0, (size[i] - 1) as f32);
        let below = e.floor() as usize;
        let above = (below + 1).min(size[i] - 1);
        corners.push((below, above, e - below as f32));
    }
    let mut out = vec![0.0; outputs];
    // Every corner of the cell `x` is in, weighted by how near it is.
    for corner in 0..1_usize << inputs {
        let mut weight = 1.0;
        let mut index = 0;
        let mut stride = 1;
        for (i, &(below, above, t)) in corners.iter().enumerate() {
            let (at, w) = if corner >> i & 1 == 1 { (above, t) } else { (below, 1.0 - t) };
            weight *= w;
            index += at * stride;
            stride *= size[i];
        }
        if weight == 0.0 {
            continue;
        }
        for (o, value) in out.iter_mut().enumerate() {
            *value += weight * samples.get(index * outputs + o).copied().unwrap_or(0.0);
        }
    }
    out
}

/// A PostScript calculator operation.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    Push(Value),
    /// `{ then } if`, or `{ then } { otherwise } ifelse`.
    If(Vec<Op>, Option<Vec<Op>>),
    Operator(Operator),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Value {
    Number(f32),
    Bool(bool),
}

impl Value {
    fn number(self) -> Option<f32> {
        match self {
            Value::Number(n) => Some(n),
            Value::Bool(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Operator {
    Abs, Add, Atan, Ceiling, Cos, Cvi, Cvr, Div, Exp, Floor, Idiv, Ln, Log, Mod, Mul, Neg, Round, Sin, Sqrt, Sub, Truncate,
    And, Bitshift, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, Xor,
    Copy, Dup, Exch, Index, Pop, Roll,
}

/// A calculator program, `{ ... }`.
fn parse_postscript(program: &[u8]) -> Option<Vec<Op>> {
    let text = std::str::from_utf8(program).ok()?;
    let spaced = text.replace('{', " { ").replace('}', " } ");
    let mut tokens = spaced.split_whitespace();
    if tokens.next()? != "{" {
        return None;
    }
    parse_block(&mut tokens)
}

/// The operations up to the `}` closing a block whose `{` has been read.
fn parse_block<'a>(tokens: &mut impl Iterator<Item = &'a str>) -> Option<Vec<Op>> {
    let mut ops = Vec::new();
    // Blocks read but not yet taken by `if` or `ifelse`.
    let mut blocks: Vec<Vec<Op>> = Vec::new();
    loop {
        let token = tokens.next()?;
        let op = match token {
            "}" => return blocks.is_empty().then_some(ops),
            "{" => {
                blocks.push(parse_block(tokens)?);
                continue;
            }
            "if" => {
                let then = blocks.pop()?;
                Op::If(then, None)
            }
            "ifelse" => {
                let otherwise = blocks.pop()?;
                let then = blocks.pop()?;
                Op::If(then, Some(otherwise))
            }
            "true" => Op::Push(Value::Bool(true)),
            "false" => Op::Push(Value::Bool(false)),
            _ => match token.parse::<f32>() {
                Ok(n) => Op::Push(Value::Number(n)),
                Err(_) => Op::Operator(operator(token)?),
            },
        };
        if !blocks.is_empty() {
            return None;
        }
        ops.push(op);
    }
}

fn operator(name: &str) -> Option<Operator> {
    use Operator::*;
    Some(match name {
        "abs" => Abs, "add" => Add, "atan" => Atan, "ceiling" => Ceiling, "cos" => Cos, "cvi" => Cvi, "cvr" => Cvr,
        "div" => Div, "exp" => Exp, "floor" => Floor, "idiv" => Idiv, "ln" => Ln, "log" => Log, "mod" => Mod,
        "mul" => Mul, "neg" => Neg, "round" => Round, "sin" => Sin, "sqrt" => Sqrt, "sub" => Sub, "truncate" => Truncate,
        "and" => And, "bitshift" => Bitshift, "eq" => Eq, "ge" => Ge, "gt" => Gt, "le" => Le, "lt" => Lt, "ne" => Ne,
        "not" => Not, "or" => Or, "xor" => Xor,
        "copy" => Copy, "dup" => Dup, "exch" => Exch, "index" => Index, "pop" => Pop, "roll" => Roll,
        _ => return None,
    })
}

/// Runs `program` on `stack`; `None` if it goes wrong, as a stack running
/// out does.
fn run(program: &[Op], stack: &mut Vec<Value>) -> Option<()> {
    use Operator::*;
    for op in program {
        match op {
            Op::Push(value) => stack.push(*value),
            Op::If(then, otherwise) => {
                let Value::Bool(condition) = stack.pop()? else { return None };
                if condition {
                    run(then, stack)?;
                } else if let Some(otherwise) = otherwise {
                    run(otherwise, stack)?;
                }
            }
            Op::Operator(operator) => {
                let num = |stack: &mut Vec<Value>| stack.pop().and_then(Value::number);
                let int = |stack: &mut Vec<Value>| num(stack).map(|n| n as i64);
                match operator {
                    Abs | Ceiling | Cos | Cvi | Cvr | Floor | Ln | Log | Neg | Round | Sin | Sqrt | Truncate => {
                        let a = num(stack)?;
                        let value = match operator {
                            Abs => a.abs(),
                            Ceiling => a.ceil(),
                            Cos => a.to_radians().cos(),
                            Cvi | Truncate => a.trunc(),
                            Cvr => a,
                            Floor => a.floor(),
                            Ln => a.ln(),
                            Log => a.log10(),
                            Neg => -a,
                            Round => (a + 0.5).floor(),
                            Sin => a.to_radians().sin(),
                            _ => a.sqrt(),
                        };
                        stack.push(Value::Number(value));
                    }
                    Add | Atan | Div | Exp | Mul | Sub => {
                        let b = num(stack)?;
                        let a = num(stack)?;
                        let value = match operator {
                            Add => a + b,
                            Atan => {
                                let degrees = a.atan2(b).to_degrees();
                                if degrees < 0.0 { degrees + 360.0 } else { degrees }
                            }
                            Div => a / b,
                            Exp => a.powf(b),
                            Mul => a * b,
                            _ => a - b,
                        };
                        stack.push(Value::Number(value));
                    }
                    Idiv | Mod | Bitshift => {
                        let b = int(stack)?;
                        let a = int(stack)?;
                        let value = match operator {
                            Idiv => a.checked_div(b)?,
                            Mod => a.checked_rem(b)?,
                            _ if b >= 0 => a.checked_shl(b as u32).unwrap_or(0),
                            _ => a.checked_shr((-b) as u32).unwrap_or(0),
                        };
                        stack.push(Value::Number(value as f32));
                    }
                    Eq | Ne | Ge | Gt | Le | Lt => {
                        let b = stack.pop()?;
                        let a = stack.pop()?;
                        let value = match (operator, a, b) {
                            (Eq, a, b) => a == b,
                            (Ne, a, b) => a != b,
                            (_, Value::Number(a), Value::Number(b)) => match operator {
                                Ge => a >= b,
                                Gt => a > b,
                                Le => a <= b,
                                _ => a < b,
                            },
                            _ => return None,
                        };
                        stack.push(Value::Bool(value));
                    }
                    And | Or | Xor => {
                        let value = match (stack.pop()?, stack.pop()?) {
                            (Value::Bool(b), Value::Bool(a)) => Value::Bool(match operator {
                                And => a & b,
                                Or => a | b,
                                _ => a ^ b,
                            }),
                            (Value::Number(b), Value::Number(a)) => {
                                let (a, b) = (a as i64, b as i64);
                                Value::Number(match operator {
                                    And => a & b,
                                    Or => a | b,
                                    _ => a ^ b,
                                } as f32)
                            }
                            _ => return None,
                        };
                        stack.push(value);
                    }
                    Not => {
                        let value = match stack.pop()? {
                            Value::Bool(b) => Value::Bool(!b),
                            Value::Number(n) => Value::Number(!(n as i64) as f32),
                        };
                        stack.push(value);
                    }
                    Copy => {
                        let n = usize::try_from(int(stack)?).ok()?;
                        let start = stack.len().checked_sub(n)?;
                        stack.extend_from_within(start..);
                    }
                    Dup => stack.push(*stack.last()?),
                    Exch => {
                        let len = stack.len();
                        if len < 2 {
                            return None;
                        }
                        stack.swap(len - 1, len - 2);
                    }
                    Index => {
                        let n = usize::try_from(int(stack)?).ok()?;
                        let value = *stack.get(stack.len().checked_sub(n + 1)?)?;
                        stack.push(value);
                    }
                    Pop => {
                        stack.pop()?;
                    }
                    Roll => {
                        let j = int(stack)?;
                        let n = usize::try_from(int(stack)?).ok()?;
                        let start = stack.len().checked_sub(n)?;
                        if n > 0 {
                            let shift = j.rem_euclid(n as i64) as usize;
                            stack[start..].rotate_right(shift);
                        }
                    }
                }
            }
        }
        if stack.len() > DEEPEST_STACK {
            return None;
        }
    }
    Some(())
}

/// An array of numbers, following references.
fn numbers(doc: &Document, object: &Object) -> Option<Vec<f32>> {
    match doc.dereference(object).ok()?.1 {
        Object::Array(items) => items.iter().map(|item| number(doc, item).map(|n| n as f32)).collect(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::lopdf::{dictionary, Stream};

    fn read(doc: &mut Document, object: Object) -> Function {
        let id = doc.add_object(object);
        Function::read(doc, &Object::Reference(id)).expect("a function")
    }

    fn close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} against {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-3, "{actual:?} against {expected:?}");
        }
    }

    #[test]
    fn a_sampled_tint_transform_interpolates_between_its_samples() {
        let mut doc = Document::with_version("1.5");
        // Tint 0 is no ink, tint 1 full black in CMYK, over three samples.
        let dict = dictionary! {
            "FunctionType" => 0, "Domain" => vec![0.into(), 1.into()], "Range" => vec![0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into()],
            "Size" => vec![3.into()], "BitsPerSample" => 8,
        };
        let f = read(&mut doc, Object::Stream(Stream::new(dict, vec![0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 255])));
        close(&f.eval(&[0.0]).unwrap(), &[0.0, 0.0, 0.0, 0.0]);
        close(&f.eval(&[0.25]).unwrap(), &[0.0, 0.0, 0.0, 128.0 / 510.0]);
        close(&f.eval(&[1.0]).unwrap(), &[0.0, 0.0, 0.0, 1.0]);
        close(&f.eval(&[7.0]).unwrap(), &[0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_sampled_function_of_two_inputs_interpolates_across_both() {
        let mut doc = Document::with_version("1.5");
        let dict = dictionary! {
            "FunctionType" => 0, "Domain" => vec![0.into(), 1.into(), 0.into(), 1.into()], "Range" => vec![0.into(), 1.into()],
            "Size" => vec![2.into(), 2.into()], "BitsPerSample" => 8,
        };
        // (0,0) 0, (1,0) 255, (0,1) 0, (1,1) 255: the first input varies fastest.
        let f = read(&mut doc, Object::Stream(Stream::new(dict, vec![0, 255, 0, 255])));
        close(&f.eval(&[0.5, 0.9]).unwrap(), &[0.5]);
        close(&f.eval(&[1.0, 0.0]).unwrap(), &[1.0]);
    }

    #[test]
    fn exponential_and_stitching_functions() {
        let mut doc = Document::with_version("1.5");
        let exponential = dictionary! { "FunctionType" => 2, "Domain" => vec![0.into(), 1.into()], "C0" => vec![1.into(), 0.into()], "C1" => vec![0.into(), 1.into()], "N" => 2 };
        let f = read(&mut doc, Object::Dictionary(exponential.clone()));
        close(&f.eval(&[0.5]).unwrap(), &[0.75, 0.25]);

        let stitching = dictionary! {
            "FunctionType" => 3, "Domain" => vec![0.into(), 1.into()], "Bounds" => vec![Object::Real(0.5)],
            "Encode" => vec![0.into(), 1.into(), 1.into(), 0.into()],
            "Functions" => vec![Object::Dictionary(exponential.clone()), Object::Dictionary(exponential)],
        };
        let f = read(&mut doc, Object::Dictionary(stitching));
        close(&f.eval(&[0.25]).unwrap(), &[0.75, 0.25]);
        close(&f.eval(&[1.0]).unwrap(), &[1.0, 0.0]);
    }

    #[test]
    fn a_postscript_calculator_runs_with_conditionals_and_the_stack() {
        let mut doc = Document::with_version("1.5");
        let dict = dictionary! { "FunctionType" => 4, "Domain" => vec![0.into(), 1.into(), 0.into(), 1.into()], "Range" => vec![0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into()] };
        // Two inks to CMYK: cyan from the first, black from the second when
        // above a half, else none.
        let program = b"{ exch 0 0 4 -1 roll dup 0.5 gt { } { pop 0 } ifelse }".to_vec();
        let f = read(&mut doc, Object::Stream(Stream::new(dict, program)));
        assert_eq!(f.inputs(), 2);
        close(&f.eval(&[0.3, 0.8]).unwrap(), &[0.3, 0.0, 0.0, 0.8]);
        close(&f.eval(&[0.3, 0.2]).unwrap(), &[0.3, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_malformed_function_is_none() {
        let mut doc = Document::with_version("1.5");
        let id = doc.add_object(dictionary! { "FunctionType" => 4, "Domain" => vec![0.into(), 1.into()], "Range" => vec![0.into(), 1.into()] });
        assert!(Function::read(&doc, &Object::Reference(id)).is_none(), "a type 4 function needs a stream");
        let dict = dictionary! { "FunctionType" => 4, "Domain" => vec![0.into(), 1.into()], "Range" => vec![0.into(), 1.into()] };
        let id = doc.add_object(Object::Stream(Stream::new(dict, b"{ 1 frobnicate }".to_vec())));
        assert!(Function::read(&doc, &Object::Reference(id)).is_none());
    }
}
