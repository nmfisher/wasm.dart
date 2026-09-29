use std::{env, fs};

use serde::Serialize;
use serde_json::value::RawValue;
use wasmtime::{
    Config, Result, Store, bail,
    component::{Component, Linker, Val},
    error::Context,
};

fn main() -> Result<()> {
    let Some(file) = env::args().nth(1) else {
        bail!("Usage: cargo run -- file.wasm")
    };

    let mut config = Config::default();
    config.wasm_gc(true);
    config.wasm_function_references(true);
    config.wasm_exceptions(true);

    let engine = wasmtime::Engine::new(&config)?;
    let mut store = Store::new(&engine, ());

    let bytes = fs::read(&file).with_context(|| format!("Reading {file}"))?;
    let component = Component::new(&engine, bytes)?;
    let Some(run_instance_index) = component.get_export_index(None, "wasmdart:tests/tested-module")
    else {
        bail!("Missing tests export");
    };
    let Some(count_test_idx) = component.get_export_index(Some(&run_instance_index), "count-tests")
    else {
        bail!("Missing count-tests");
    };
    let Some(invoke_test_idx) =
        component.get_export_index(Some(&run_instance_index), "invoke-test")
    else {
        bail!("Missing invoke-test");
    };

    let mut linker = Linker::new(&engine);
    {
        let mut root = linker.root();
        let mut collector = root.instance("wasmdart:tests/result-collector")?;
        collector.func_wrap("record-string", |_store, params: (String,)| {
            post_event(&TestEvent::RecordedString { value: params.0 });
            Ok(())
        })?;
        collector.func_wrap("record-double", |_store, params: (f64,)| {
            post_event(&TestEvent::RecordedDouble {
                value: RawValue::from_string(dart_double_json(params.0))
                    .expect("a rendered double is always valid JSON"),
            });
            Ok(())
        })?;
        collector.func_wrap("record-int", |_store, params: (i64,)| {
            post_event(&TestEvent::RecordedInt { value: params.0 });
            Ok(())
        })?;
        collector.func_wrap("record-bool", |_store, params: (bool,)| {
            post_event(&TestEvent::RecordedBool { value: params.0 });
            Ok(())
        })?;
    }

    let instance = linker.instantiate(&mut store, &component)?;
    let count = instance.get_func(&mut store, count_test_idx).unwrap();
    let invoke = instance.get_func(&mut store, invoke_test_idx).unwrap();

    let num_tests = {
        let mut results = [Val::Result(Ok(None))];
        count.call(&mut store, &[], &mut results)?;
        let Val::U32(count) = results[0] else {
            panic!();
        };
        count
    };

    for i in 0..num_tests {
        post_event(&TestEvent::TestStart { test: i });
        invoke.call(&mut store, &[Val::U32(i)], &mut []).unwrap();
        post_event(&TestEvent::TestEnd { test: i });
    }

    Ok(())
}

fn post_event(event: &TestEvent) {
    let formatted = serde_json::to_string(event).unwrap();
    println!("{formatted}")
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum TestEvent {
    #[serde(rename = "start")]
    TestStart { test: u32 },
    #[serde(rename = "end")]
    TestEnd { test: u32 },
    #[serde(rename = "string")]
    RecordedString { value: String },
    #[serde(rename = "double")]
    RecordedDouble { value: Box<RawValue> },
    #[serde(rename = "int")]
    RecordedInt { value: i64 },
    #[serde(rename = "bool")]
    RecordedBool { value: bool },
}

/// Renders a double the way the Dart VM does, so the host's output matches
/// what the native runner writes and what the golden files hold.
///
/// The guest hands over the bits of a Dart `double`. The goldens come from
/// `run_native.dart`, which does `json.encode(value)`, and on the VM that is
/// the same text as `double.toString()`: the ECMAScript `Number::toString`
/// layout (plain notation for -6 < n <= 21) plus a trailing `.0` on integral
/// values. serde_json instead formats with its own shortest-notation rule and
/// prints `1e-6` where the VM prints `0.000001`, so the text is produced here
/// rather than by serde_json.
///
/// NaN and ±Infinity have no JSON form - the VM's `json.encode` throws
/// `JsonUnsupportedObjectError` for them - and no case records one; `null` is
/// emitted so the line stays valid JSON.
fn dart_double_json(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let negative = value.is_sign_negative();
    // Rust's LowerExp formatting is the shortest decimal string that
    // round-trips, in `d[.ddd]e<exp>` form: the same digits the VM picks.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    // With value = 0.<digits> * 10^n: k is the digit count and n the decimal
    // point's position inside `digits`.
    let k = digits.len() as i32;
    let n = exponent + 1;

    let plain = if k <= n && n <= 21 {
        // Integral value: the digits, then n - k zeros, then Dart's `.0`.
        let mut s = digits.clone();
        s.extend(core::iter::repeat('0').take((n - k) as usize));
        s.push_str(".0");
        s
    } else if n > 0 && n <= 21 {
        // The point falls inside the digits: `123.456`. (n > 21 with k <= n
        // was already taken by the integral branch above, so a plain-notation
        // n can only sit inside the digits - values like `1e+23`, whose
        // shortest digits are `1` but whose point sits 24 places in, are
        // written exponentially instead.)
        let n = n as usize;
        format!("{}.{}", &digits[..n], &digits[n..])
    } else if n > -6 && n <= 0 {
        // `0.` followed by -n zeros and the digits: `0.000001`.
        let mut s = String::from("0.");
        s.extend(core::iter::repeat('0').take((-n) as usize));
        s.push_str(&digits);
        s
    } else {
        // Exponential notation: `1.5e-7`, `1e+23`.
        let exponent = n - 1;
        let sign = if exponent < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign}{}", exponent.abs())
        } else {
            format!(
                "{}.{}e{sign}{}",
                &digits[..1],
                &digits[1..],
                exponent.abs()
            )
        }
    };
    if negative {
        format!("-{plain}")
    } else {
        plain
    }
}

#[cfg(test)]
mod tests {
    use super::dart_double_json;

    /// Renderings taken from the Dart VM (`dart` 3.14.0-edge, `json.encode`
    /// and `toString()` agreeing for every finite value). Each entry is the
    /// VM's own output, so parsing it back must yield the original bits.
    const VM_RENDERINGS: &[&str] = &[
        "0.0",
        "-0.0",
        "1.0",
        "-1.0",
        "1.5",
        "-1.5",
        "0.5",
        "2.25",
        "123.0",
        "12.5",
        "3.141592653589793",
        "1000.0",
        "0.001",
        "0.0015",
        "-250.0",
        "5.0",
        "0.000001",
        "1e+23",
        "-1000.0",
        "123.456",
        "-0.5",
        "20000000000.0",
        "100000000000000000000.0",
        "1e+21",
        "1e+22",
        "1e-7",
        "1.5e-7",
        "10000000000000000.0",
        "1.5e+300",
        "5e-324",
        "1.7976931348623157e+308",
        "2.2250738585072014e-308",
        "0.1",
        "0.2",
        "0.3",
        "0.3333333333333333",
        "1234567890123456.0",
        "1000000000000000.0",
        "100000000000000000.0",
        "0.00001",
        "0.0001",
        "9.95e-320",
        "1e-323",
        "2.5e-323",
        "100000.0",
        "1.0000000000000002",
        "5e-7",
        "1e-20",
        "6.02e+23",
        "-1e-7",
        "-1e+21",
        "100.0",
    ];

    #[test]
    fn renders_like_the_dart_vm() {
        for expected in VM_RENDERINGS {
            let value: f64 = expected.parse().expect("VM rendering is a valid literal");
            assert_eq!(
                dart_double_json(value),
                *expected,
                "value {expected} (bits {value:?})"
            );
        }
    }

    #[test]
    fn matches_every_double_in_double_parse_golden() {
        // The 26 doubles `double_parse.golden.txt` holds, in order.
        let golden = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../wasm_components/test/cases/double_parse.golden.txt"
        ))
        .unwrap();
        let recorded: Vec<&str> = golden
            .lines()
            .filter_map(|line| {
                line.strip_prefix(r#"{"type":"double","value":"#)
                    .map(|rest| rest.strip_suffix('}').expect("line ends with }"))
            })
            .collect();
        assert_eq!(recorded.len(), 26, "the golden's doubles");
        for value in recorded {
            let parsed: f64 = value.parse().unwrap();
            assert_eq!(dart_double_json(parsed), value, "golden value {value}");
        }
    }

    #[test]
    fn non_finite_values_have_no_json_form() {
        // The VM's `json.encode` throws for these; there is no Dart text to
        // reproduce, so the host emits JSON's null.
        assert_eq!(dart_double_json(f64::NAN), "null");
        assert_eq!(dart_double_json(f64::INFINITY), "null");
        assert_eq!(dart_double_json(f64::NEG_INFINITY), "null");
    }
}
