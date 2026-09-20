use crate::{ActError, Result, Vars};
use rquickjs::{
    Array as JsArray, BigInt as JsBigInt, FromJs, Function as JsFunction, IntoAtom, IntoJs,
    Object as JsObject, String as JsString, Value as JsValue,
};
use serde::de::DeserializeOwned;

/// Largest integer a JS number (a f64) carries exactly. Wider values cross
/// the boundary as BigInt: a double would silently drop their low digits.
const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991; // 2^53 - 1

/// `JS_NewFloat64` keeps doubles that fit an i32 as ints, and every JS number
/// is a double. Mirror that: an integral value inside the exact range becomes
/// a JSON integer, so an `i64`/`u64` round-trips unchanged instead of turning
/// into `3000000000.0`, which no typed field accepts. Wider values stay
/// floats — they were doubles in JS, and re-entering as BigInt would change
/// their type from underneath scripts doing arithmetic on them.
fn json_number_from_float(v: f64) -> rquickjs::Result<serde_json::Value> {
    if !v.is_finite() {
        return Err(rquickjs::Error::new_from_js_message(
            "number",
            "JSON value",
            "non-finite numbers have no JSON representation",
        ));
    }
    if v.fract() == 0.0 && v.abs() <= JS_MAX_SAFE_INTEGER as f64 {
        return Ok(serde_json::json!(v as i64));
    }
    Ok(serde_json::json!(v))
}

/// An `i64` reaches JS as an int when it fits, as a double while a double
/// carries it exactly, and as a BigInt beyond that. Narrowing to `i32` would
/// corrupt timestamps, ids, and counters; rounding to a double would corrupt
/// anything past 2^53.
fn js_number_from_i64<'js>(ctx: &rquickjs::Ctx<'js>, v: i64) -> rquickjs::Result<JsValue<'js>> {
    if let Ok(v) = i32::try_from(v) {
        Ok(JsValue::new_int(ctx.clone(), v))
    } else if (-JS_MAX_SAFE_INTEGER..=JS_MAX_SAFE_INTEGER).contains(&v) {
        Ok(JsValue::new_float(ctx.clone(), v as f64))
    } else {
        Ok(JsValue::from_big_int(JsBigInt::from_i64(ctx.clone(), v)?))
    }
}

#[derive(Debug)]
pub struct ActJsValue(serde_json::Value);

impl ActJsValue {
    pub fn new(v: serde_json::Value) -> Self {
        Self(v)
    }

    pub fn inner(&self) -> &serde_json::Value {
        &self.0
    }

    pub fn to<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value::<T>(self.0.clone()).map_err(|err| ActError::Script(err.to_string()))
    }
}

impl<'js> IntoJs<'js> for ActJsValue {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let value = match self.0 {
            serde_json::Value::Null => JsValue::new_null(ctx.clone()),
            serde_json::Value::Bool(v) => JsValue::new_bool(ctx.clone(), v),
            serde_json::Value::Number(v) => {
                if let Some(v) = v.as_i64() {
                    js_number_from_i64(ctx, v)?
                } else if let Some(v) = v.as_u64() {
                    // `as_i64` already covers everything up to `i64::MAX`, so
                    // the u64 side is always wider than a double's exact range.
                    JsValue::from_big_int(JsBigInt::from_u64(ctx.clone(), v)?)
                } else {
                    // Every remaining serde_json number is an f64; NaN and the
                    // infinities never make it into one.
                    JsValue::new_float(ctx.clone(), v.as_f64().unwrap_or_default())
                }
            }
            serde_json::Value::String(v) => {
                JsValue::from_string(JsString::from_str(ctx.clone(), &v)?)
            }
            serde_json::Value::Array(v) => {
                let arr = JsArray::new(ctx.clone())?;
                for (idx, v) in v.iter().enumerate() {
                    let val = ActJsValue(v.clone()).into_js(ctx)?;
                    arr.set(idx, val)?;
                }
                JsValue::from_array(arr)
            }
            serde_json::Value::Object(v) => {
                let obj = JsObject::new(ctx.clone())?;
                for (k, v) in v {
                    obj.set(k.into_atom(ctx)?, ActJsValue(v).into_js(ctx)?)?;
                }

                JsValue::from_object(obj)
            }
        };

        Ok(value)
    }
}

impl<'js> FromJs<'js> for ActJsValue {
    fn from_js(ctx: &rquickjs::Ctx<'js>, v: JsValue<'js>) -> rquickjs::Result<Self> {
        let result = match v.type_of() {
            rquickjs::Type::Null | rquickjs::Type::Undefined | rquickjs::Type::Uninitialized => {
                Ok(serde_json::json!(null))
            }
            rquickjs::Type::Bool => Ok(serde_json::json!(v.as_bool().unwrap_or(false))),
            rquickjs::Type::Int => Ok(serde_json::json!(v.as_int().unwrap_or(0))),
            rquickjs::Type::Float => json_number_from_float(v.as_float().unwrap_or(0.0)),
            rquickjs::Type::String => Ok(serde_json::json!(
                v.as_string()
                    .ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            v.type_name(),
                            "string",
                            "expected a JS string",
                        )
                    })?
                    .to_string()?
            )),
            rquickjs::Type::Array => {
                let array = v.as_array().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "Array",
                        "expected a JS array",
                    )
                })?;
                let values = array
                    .iter::<JsValue>()
                    .map(|item| {
                        let item = item?;
                        Ok(ActJsValue::from_js(ctx, item)?.into())
                    })
                    .collect::<rquickjs::Result<Vec<_>>>()?;
                Ok(serde_json::Value::Array(values))
            }
            // rquickjs reports a `Proxy` as its own type, but `as_object`
            // accepts it, so its properties (through the traps) read like a
            // plain object's.
            rquickjs::Type::Object | rquickjs::Type::Proxy => {
                let mut value = serde_json::Map::<String, serde_json::Value>::new();
                let inner = JsObject::new(ctx.clone())?;
                let object = v.as_object().unwrap_or(&inner);
                let keys = object
                    .keys::<String>()
                    .filter_map(|v| v.ok())
                    .collect::<Vec<_>>();
                let values = keys
                    .iter()
                    .filter_map(|key| {
                        match object.get::<String, JsValue>(key.clone()) {
                            Ok(value) => Ok((key, value)),
                            Err(err) => Err(err),
                        }
                        .ok()
                    })
                    .collect::<Vec<_>>();
                for (k, v) in values {
                    value.insert(k.clone(), ActJsValue::from_js(ctx, v)?.into());
                }
                Ok(serde_json::Value::Object(value))
            }
            rquickjs::Type::BigInt => {
                let bigint = v.as_big_int().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "BigInt",
                        "expected a BigInt",
                    )
                })?;

                // `JS_ToInt64` wraps oversized BigInts, so derive its canonical
                // decimal representation and range-check it before narrowing.
                // Positive values land on the `u64` side of `serde_json::Number`,
                // which is what `IntoJs` emits for values past `i64::MAX`.
                let to_string = ctx.globals().get::<_, JsFunction>("String")?;
                let text = to_string
                    .call::<_, JsValue>((bigint.clone(),))?
                    .as_string()
                    .ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            v.type_name(),
                            "string",
                            "cannot stringify a BigInt",
                        )
                    })?
                    .to_string()?;
                let negative = text.starts_with('-');
                let digits = text.trim_start_matches('-');
                let magnitude: i128 = digits.parse().map_err(|_| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "i64",
                        "invalid BigInt value",
                    )
                })?;
                let signed = if negative { -magnitude } else { magnitude };
                let outside = || {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "i64",
                        "BigInt value is outside the i64/u64 range",
                    )
                };
                if let Ok(v) = i64::try_from(signed) {
                    Ok(serde_json::json!(v))
                } else if let Ok(v) = u64::try_from(signed) {
                    Ok(serde_json::json!(v))
                } else {
                    Err(outside())
                }
            }
            rquickjs::Type::Exception => {
                let ex = v
                    .as_exception()
                    .ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            v.type_name(),
                            "Exception",
                            "expected a JS exception",
                        )
                    })?
                    .clone();
                Err(ex.throw())
            }
            rquickjs::Type::Unknown
            | rquickjs::Type::Module
            | rquickjs::Type::Constructor
            | rquickjs::Type::Symbol
            | rquickjs::Type::Function
            | rquickjs::Type::Promise => Err(rquickjs::Error::new_from_js_message(
                v.type_name(),
                "",
                "cannot convert js to json value",
            )),
        }?;

        Ok(ActJsValue(result))
    }
}

impl From<ActJsValue> for serde_json::Value {
    fn from(val: ActJsValue) -> Self {
        val.0
    }
}

impl From<Vars> for ActJsValue {
    fn from(value: Vars) -> Self {
        ActJsValue(value.into())
    }
}

impl From<String> for ActJsValue {
    fn from(value: String) -> Self {
        ActJsValue(value.into())
    }
}
