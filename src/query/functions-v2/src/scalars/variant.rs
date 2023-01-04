// Copyright 2022 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::borrow::Cow;

use bstr::ByteSlice;
use chrono::Datelike;
use common_arrow::arrow::temporal_conversions::EPOCH_DAYS_FROM_CE;
use common_expression::types::date::string_to_date;
use common_expression::types::nullable::NullableColumn;
use common_expression::types::nullable::NullableDomain;
use common_expression::types::number::Float64Type;
use common_expression::types::number::Int64Type;
use common_expression::types::number::UInt32Type;
use common_expression::types::number::UInt64Type;
use common_expression::types::timestamp::string_to_timestamp;
use common_expression::types::variant::cast_scalar_to_variant;
use common_expression::types::variant::cast_scalars_to_variants;
use common_expression::types::variant::JSONB_NULL;
use common_expression::types::BooleanType;
use common_expression::types::DataType;
use common_expression::types::DateType;
use common_expression::types::GenericType;
use common_expression::types::NullableType;
use common_expression::types::NumberDataType;
use common_expression::types::NumberType;
use common_expression::types::StringType;
use common_expression::types::TimestampType;
use common_expression::types::VariantType;
use common_expression::types::ALL_NUMERICS_TYPES;
use common_expression::utils::arrow::constant_bitmap;
use common_expression::vectorize_1_arg;
use common_expression::vectorize_2_arg;
use common_expression::vectorize_with_builder_1_arg;
use common_expression::vectorize_with_builder_2_arg;
use common_expression::with_number_mapped_type;
use common_expression::FunctionDomain;
use common_expression::FunctionProperty;
use common_expression::FunctionRegistry;
use common_expression::Value;
use common_expression::ValueRef;
use common_jsonb::array_length;
use common_jsonb::as_bool;
use common_jsonb::as_f64;
use common_jsonb::as_i64;
use common_jsonb::as_str;
use common_jsonb::get_by_name_ignore_case;
use common_jsonb::get_by_path;
use common_jsonb::is_array;
use common_jsonb::is_object;
use common_jsonb::object_keys;
use common_jsonb::parse_json_path;
use common_jsonb::parse_value;
use common_jsonb::to_bool;
use common_jsonb::to_f64;
use common_jsonb::to_i64;
use common_jsonb::to_str;
use common_jsonb::to_string;
use common_jsonb::to_u64;
use common_jsonb::JsonPath;
use common_jsonb::Number as JsonbNumber;
use common_jsonb::Value as JsonbValue;

use super::comparison::ALL_COMP_FUNC_NAMES;
use super::comparison::ALL_MATCH_FUNC_NAMES;

pub fn register(registry: &mut FunctionRegistry) {
    register_compare_functions(registry);
    register_match_functions(registry);

    registry.register_passthrough_nullable_1_arg::<StringType, VariantType, _, _>(
        "parse_json",
        FunctionProperty::default(),
        |_| FunctionDomain::MayThrow,
        vectorize_with_builder_1_arg::<StringType, VariantType>(|s, output, _| {
            if s.trim().is_empty() {
                output.put_slice(JSONB_NULL);
                output.commit_row();
                return Ok(());
            }
            let value = parse_value(s).map_err(|err| {
                format!("unable to parse '{}': {}", &String::from_utf8_lossy(s), err)
            })?;
            value.write_to_vec(&mut output.data);
            output.commit_row();

            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<StringType, VariantType, _, _>(
        "try_parse_json",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<StringType, NullableType<VariantType>>(|s, output, _| {
            if s.trim().is_empty() {
                output.push(JSONB_NULL);
            } else {
                match parse_value(s) {
                    Ok(value) => {
                        output.validity.push(true);
                        value.write_to_vec(&mut output.builder.data);
                        output.builder.commit_row();
                    }
                    Err(_) => output.push_null(),
                }
            }
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<StringType, StringType, _, _>(
        "check_json",
        FunctionProperty::default(),
        |_| FunctionDomain::MayThrow,
        vectorize_with_builder_1_arg::<StringType, NullableType<StringType>>(|s, output, _| {
            if s.trim().is_empty() {
                output.push_null();
            } else {
                match parse_value(s) {
                    Ok(_) => output.push_null(),
                    Err(e) => output.push(e.to_string().as_bytes()),
                }
            }
            Ok(())
        }),
    );

    registry.register_passthrough_nullable_1_arg::<BooleanType, VariantType, _, _>(
        "parse_json",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<BooleanType, VariantType>(|s, output, _| {
            let value = if s {
                JsonbValue::Bool(true)
            } else {
                JsonbValue::Bool(false)
            };
            value.write_to_vec(&mut output.data);
            output.commit_row();
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<BooleanType, VariantType, _, _>(
        "try_parse_json",
        FunctionProperty::default(),
        |_| {
            FunctionDomain::Domain(NullableDomain {
                has_null: false,
                value: Some(Box::new(())),
            })
        },
        vectorize_with_builder_1_arg::<BooleanType, NullableType<VariantType>>(|s, output, _| {
            let value = if s {
                JsonbValue::Bool(true)
            } else {
                JsonbValue::Bool(false)
            };
            output.validity.push(true);
            value.write_to_vec(&mut output.builder.data);
            output.builder.commit_row();
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<BooleanType, StringType, _, _>(
        "check_json",
        FunctionProperty::default(),
        |_| {
            FunctionDomain::Domain(NullableDomain {
                has_null: true,
                value: None,
            })
        },
        vectorize_with_builder_1_arg::<BooleanType, NullableType<StringType>>(|_, output, _| {
            output.push_null();
            Ok(())
        }),
    );

    for src_type in ALL_NUMERICS_TYPES {
        with_number_mapped_type!(|NUM_TYPE| match src_type {
            NumberDataType::NUM_TYPE => {
                registry
                    .register_passthrough_nullable_1_arg::<NumberType<NUM_TYPE>, VariantType, _, _>(
                        "parse_json",
                        FunctionProperty::default(),
                        |_| FunctionDomain::Full,
                        vectorize_with_builder_1_arg::<NumberType<NUM_TYPE>, VariantType>(
                            move |s, output, _| {
                                let value = if src_type.is_float() {
                                    let v = num_traits::cast::cast(s).unwrap();
                                    JsonbValue::Number(JsonbNumber::Float64(v))
                                } else if src_type.is_signed() {
                                    let v = num_traits::cast::cast(s).unwrap();
                                    JsonbValue::Number(JsonbNumber::Int64(v))
                                } else {
                                    let v = num_traits::cast::cast(s).unwrap();
                                    JsonbValue::Number(JsonbNumber::UInt64(v))
                                };
                                value.write_to_vec(&mut output.data);
                                output.commit_row();
                                Ok(())
                            },
                        ),
                    );

                registry
                    .register_combine_nullable_1_arg::<NumberType<NUM_TYPE>, VariantType, _, _>(
                        "try_parse_json",
                        FunctionProperty::default(),
                        |_| {
                            FunctionDomain::Domain(NullableDomain {
                                has_null: false,
                                value: Some(Box::new(())),
                            })
                        },
                        vectorize_with_builder_1_arg::<
                            NumberType<NUM_TYPE>,
                            NullableType<VariantType>,
                        >(move |s, output, _ctx| {
                            let value = if src_type.is_float() {
                                let v = num_traits::cast::cast(s).unwrap();
                                JsonbValue::Number(JsonbNumber::Float64(v))
                            } else if src_type.is_signed() {
                                let v = num_traits::cast::cast(s).unwrap();
                                JsonbValue::Number(JsonbNumber::Int64(v))
                            } else {
                                let v = num_traits::cast::cast(s).unwrap();
                                JsonbValue::Number(JsonbNumber::UInt64(v))
                            };
                            output.validity.push(true);
                            value.write_to_vec(&mut output.builder.data);
                            output.builder.commit_row();
                            Ok(())
                        }),
                    );

                registry.register_combine_nullable_1_arg::<NumberType<NUM_TYPE>, StringType, _, _>(
                    "check_json",
                    FunctionProperty::default(),
                    |_| {
                        FunctionDomain::Domain(NullableDomain {
                            has_null: true,
                            value: None,
                        })
                    },
                    vectorize_with_builder_1_arg::<NumberType<NUM_TYPE>, NullableType<StringType>>(
                        |_, output, _| {
                            output.push_null();
                            Ok(())
                        },
                    ),
                );
            }
        });
    }

    registry.register_1_arg_core::<NullableType<VariantType>, NullableType<UInt32Type>, _, _>(
        "length",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_1_arg::<NullableType<VariantType>, NullableType<UInt32Type>>(|val, _| {
            val.and_then(|v| array_length(v).map(|v| v as u32))
        }),
    );

    registry.register_1_arg_core::<NullableType<VariantType>, NullableType<VariantType>, _, _>(
        "object_keys",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_1_arg::<NullableType<VariantType>, NullableType<VariantType>>(|val, _| {
            val.and_then(object_keys)
        }),
    );

    registry.register_2_arg_core::<NullableType<VariantType>, NullableType<UInt64Type>, NullableType<VariantType>, _, _>(
        "get",
        FunctionProperty::default(),
        |_, _| FunctionDomain::Full,
        vectorize_2_arg::<NullableType<VariantType>, NullableType<UInt64Type>, NullableType<VariantType>>(|val, idx, _| {
            match (val, idx) {
                (Some(val), Some(idx)) => {
                    if val.is_empty() {
                        None
                    } else {
                        let json_path = JsonPath::UInt64(idx);
                        get_by_path(val, vec![json_path])
                    }
                }
                (_, _) => None,
            }
        }),
    );

    registry.register_2_arg_core::<NullableType<VariantType>, NullableType<StringType>, NullableType<VariantType>, _, _>(
        "get",
        FunctionProperty::default(),
        |_, _| FunctionDomain::MayThrow,
        vectorize_2_arg::<NullableType<VariantType>, NullableType<StringType>, NullableType<VariantType>>(|val, name, _| {
            match (val, name) {
                (Some(val), Some(name)) => {
                    if val.is_empty() || name.trim().is_empty() {
                        None
                    } else {
                        let name = String::from_utf8(name.to_vec()).map_err(|err| {
                            format!(
                                "Unable convert name '{}' to string: {}",
                                &String::from_utf8_lossy(name),
                                err
                            )
                        }).ok()?;
                        let json_path = JsonPath::String(Cow::Borrowed(&name));
                        get_by_path(val, vec![json_path])
                    }
                }
                (_, _) => None,
            }
        }),
    );

    registry.register_2_arg_core::<NullableType<VariantType>, NullableType<StringType>, NullableType<VariantType>, _, _>(
        "get_ignore_case",
        FunctionProperty::default(),
        |_, _| FunctionDomain::MayThrow,
        vectorize_2_arg::<NullableType<VariantType>, NullableType<StringType>, NullableType<VariantType>>(|val, name, _| {
            match (val, name) {
                (Some(val), Some(name)) => {
                    if val.is_empty() || name.trim().is_empty() {
                        None
                    } else {
                        let name = String::from_utf8(name.to_vec()).map_err(|err| {
                            format!(
                                "Unable convert name '{}' to string: {}",
                                &String::from_utf8_lossy(name),
                                err
                            )
                        }).ok()?;
                        get_by_name_ignore_case(val, &name)
                    }
                }
                (_, _) => None,
            }
        }),
    );

    registry.register_2_arg_core::<NullableType<VariantType>, NullableType<StringType>, NullableType<VariantType>, _, _>(
        "get_path",
        FunctionProperty::default(),
        |_, _| FunctionDomain::MayThrow,
        vectorize_2_arg::<NullableType<VariantType>, NullableType<StringType>, NullableType<VariantType>>(|val, path, _| {
            match (val, path) {
                (Some(val), Some(path)) => {
                    if val.is_empty() || path.is_empty() {
                        None
                    } else {
                        let json_paths = parse_json_path(path).map_err(|err| {
                            format!(
                                "Invalid extraction path '{}': {}",
                                &String::from_utf8_lossy(path),
                                err
                            )
                        }).ok()?;
                        get_by_path(val, json_paths)
                    }
                }
                (_, _) => None,
            }
        }),
    );

    registry.register_combine_nullable_2_arg::<StringType, StringType, StringType, _, _>(
        "json_extract_path_text",
        FunctionProperty::default(),
        |_, _| FunctionDomain::MayThrow,
        vectorize_with_builder_2_arg::<StringType, StringType, NullableType<StringType>>(
            |s, path, output, _| {
                if s.is_empty() || path.is_empty() {
                    output.push_null();
                } else {
                    let value = common_jsonb::parse_value(s).map_err(|err| {
                        format!("unable to parse '{}': {}", &String::from_utf8_lossy(s), err)
                    })?;

                    let json_paths = parse_json_path(path).map_err(|err| {
                        format!(
                            "Invalid extraction path '{}': {}",
                            &String::from_utf8_lossy(path),
                            err
                        )
                    })?;
                    match value.get_by_path(&json_paths) {
                        Some(val) => {
                            let json_val = format!("{val}");
                            output.push(json_val.as_bytes());
                        }
                        None => output.push_null(),
                    }
                }
                Ok(())
            },
        ),
    );

    registry.register_combine_nullable_1_arg::<VariantType, BooleanType, _, _>(
        "as_boolean",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<BooleanType>>(|v, output, _| {
            if v.is_empty() {
                output.push_null();
            } else {
                match as_bool(v) {
                    Some(val) => output.push(val),
                    None => output.push_null(),
                }
            }
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, Int64Type, _, _>(
        "as_integer",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<Int64Type>>(|v, output, _| {
            if v.is_empty() {
                output.push_null();
            } else {
                match as_i64(v) {
                    Some(val) => output.push(val),
                    None => output.push_null(),
                }
            }
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, Float64Type, _, _>(
        "as_float",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<Float64Type>>(|v, output, _| {
            if v.is_empty() {
                output.push_null();
            } else {
                match as_f64(v) {
                    Some(val) => output.push(val.into()),
                    None => output.push_null(),
                }
            }
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, StringType, _, _>(
        "as_string",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<StringType>>(|v, output, _| {
            if v.is_empty() {
                output.push_null();
            } else {
                match as_str(v) {
                    Some(val) => output.push(val.as_bytes()),
                    None => output.push_null(),
                }
            }
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, VariantType, _, _>(
        "as_array",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<VariantType>>(|v, output, _| {
            if v.is_empty() {
                output.push_null()
            } else if is_array(v) {
                output.push(v.as_bytes());
            } else {
                output.push_null()
            }
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, VariantType, _, _>(
        "as_object",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<VariantType>>(|v, output, _| {
            if v.is_empty() {
                output.push_null()
            } else if is_object(v) {
                output.push(v.as_bytes());
            } else {
                output.push_null()
            }
            Ok(())
        }),
    );

    registry.register_passthrough_nullable_1_arg::<GenericType<0>, VariantType, _, _>(
        "to_variant",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        |val, ctx| match val {
            ValueRef::Scalar(scalar) => {
                let mut buf = Vec::new();
                cast_scalar_to_variant(scalar, ctx.tz, &mut buf);
                Ok(Value::Scalar(buf))
            }
            ValueRef::Column(col) => {
                let new_col = cast_scalars_to_variants(col.iter(), ctx.tz);
                Ok(Value::Column(new_col))
            }
        },
    );

    registry.register_combine_nullable_1_arg::<GenericType<0>, VariantType, _, _>(
        "try_to_variant",
        FunctionProperty::default(),
        |_| {
            FunctionDomain::Domain(NullableDomain {
                has_null: false,
                value: Some(Box::new(())),
            })
        },
        |val, ctx| match val {
            ValueRef::Scalar(scalar) => {
                let mut buf = Vec::new();
                cast_scalar_to_variant(scalar, ctx.tz, &mut buf);
                Ok(Value::Scalar(Some(buf)))
            }
            ValueRef::Column(col) => {
                let new_col = cast_scalars_to_variants(col.iter(), ctx.tz);
                Ok(Value::Column(NullableColumn {
                    validity: constant_bitmap(true, new_col.len()).into(),
                    column: new_col,
                }))
            }
        },
    );

    registry.register_passthrough_nullable_1_arg::<VariantType, BooleanType, _, _>(
        "to_boolean",
        FunctionProperty::default(),
        |_| FunctionDomain::MayThrow,
        vectorize_with_builder_1_arg::<VariantType, BooleanType>(|val, output, _| {
            let value = to_bool(val)
                .map_err(|_| format!("unable to cast {} to Boolean", to_string(val)))?;
            output.push(value);
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, BooleanType, _, _>(
        "try_to_boolean",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<BooleanType>>(|val, output, _| {
            match to_bool(val) {
                Ok(value) => {
                    output.validity.push(true);
                    output.builder.push(value);
                }
                Err(_) => output.push_null(),
            }
            Ok(())
        }),
    );

    registry.register_passthrough_nullable_1_arg::<VariantType, StringType, _, _>(
        "to_string",
        FunctionProperty::default(),
        |_| FunctionDomain::MayThrow,
        vectorize_with_builder_1_arg::<VariantType, StringType>(|val, output, _| {
            let value =
                to_str(val).map_err(|_| format!("unable to cast {} to String", to_string(val)))?;
            output.put_slice(value.as_bytes());
            output.commit_row();
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, StringType, _, _>(
        "try_to_string",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<StringType>>(|val, output, _| {
            match to_str(val) {
                Ok(value) => {
                    output.validity.push(true);
                    output.builder.put_slice(value.as_bytes());
                    output.builder.commit_row();
                }
                Err(_) => output.push_null(),
            }
            Ok(())
        }),
    );

    registry.register_passthrough_nullable_1_arg::<VariantType, DateType, _, _>(
        "to_date",
        FunctionProperty::default(),
        |_| FunctionDomain::MayThrow,
        vectorize_with_builder_1_arg::<VariantType, DateType>(|val, output, ctx| {
            let str_val = as_str(val)
                .ok_or_else(|| format!("unable to cast {} to DateType", to_string(val),))?;
            let d = string_to_date(str_val.to_string(), ctx.tz)
                .ok_or_else(|| format!("unable to cast {} to DateType", to_string(val)))?;
            output.push(d.num_days_from_ce() - EPOCH_DAYS_FROM_CE);
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, DateType, _, _>(
        "try_to_date",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<DateType>>(|val, output, ctx| {
            match as_str(val) {
                Some(str_val) => {
                    let date = string_to_date(str_val.to_string(), ctx.tz)
                        .map(|d| (d.num_days_from_ce() - EPOCH_DAYS_FROM_CE));
                    match date {
                        Some(date) => output.push(date),
                        None => output.push_null(),
                    }
                }
                None => output.push_null(),
            }
            Ok(())
        }),
    );

    registry.register_passthrough_nullable_1_arg::<VariantType, TimestampType, _, _>(
        "to_timestamp",
        FunctionProperty::default(),
        |_| FunctionDomain::MayThrow,
        vectorize_with_builder_1_arg::<VariantType, TimestampType>(|val, output, ctx| {
            let str_val = as_str(val)
                .ok_or_else(|| format!("unable to cast {} to TimestampType", to_string(val),))?;
            let ts = string_to_timestamp(str_val.to_string(), ctx.tz)
                .ok_or_else(|| format!("unable to cast {} to TimestampType", to_string(val)))?;
            output.push(ts.timestamp_micros());
            Ok(())
        }),
    );

    registry.register_combine_nullable_1_arg::<VariantType, TimestampType, _, _>(
        "try_to_timestamp",
        FunctionProperty::default(),
        |_| FunctionDomain::Full,
        vectorize_with_builder_1_arg::<VariantType, NullableType<TimestampType>>(
            |val, output, ctx| {
                match as_str(val) {
                    Some(str_val) => {
                        let timestamp = string_to_timestamp(str_val.to_string(), ctx.tz)
                            .map(|ts| ts.timestamp_micros());
                        match timestamp {
                            Some(timestamp) => output.push(timestamp),
                            None => output.push_null(),
                        }
                    }
                    None => output.push_null(),
                }
                Ok(())
            },
        ),
    );

    for dest_type in ALL_NUMERICS_TYPES {
        with_number_mapped_type!(|NUM_TYPE| match dest_type {
            NumberDataType::NUM_TYPE => {
                let name = format!("to_{dest_type}").to_lowercase();
                registry
                    .register_passthrough_nullable_1_arg::<VariantType, NumberType<NUM_TYPE>, _, _>(
                        &name,
                        FunctionProperty::default(),
                        |_| FunctionDomain::MayThrow,
                        vectorize_with_builder_1_arg::<VariantType, NumberType<NUM_TYPE>>(
                            move |val, output, _| {
                                let value = if dest_type.is_float() {
                                    let value = to_f64(val).map_err(|_| {
                                        format!(
                                            "unable to cast {} to {}",
                                            to_string(val),
                                            dest_type
                                        )
                                    })?;
                                    num_traits::cast::cast(value).ok_or_else(|| {
                                        format!(
                                            "unable to cast {} to {}",
                                            to_string(val),
                                            dest_type,
                                        )
                                    })?
                                } else if dest_type.is_signed() {
                                    let value = to_i64(val).map_err(|_| {
                                        format!(
                                            "unable to cast {} to {}",
                                            to_string(val),
                                            dest_type
                                        )
                                    })?;
                                    num_traits::cast::cast(value).ok_or_else(|| {
                                        format!(
                                            "unable to cast {} to {}",
                                            to_string(val),
                                            dest_type,
                                        )
                                    })?
                                } else {
                                    let value = to_u64(val).map_err(|_| {
                                        format!(
                                            "unable to cast {} to {}",
                                            to_string(val),
                                            dest_type
                                        )
                                    })?;
                                    num_traits::cast::cast(value).ok_or_else(|| {
                                        format!(
                                            "unable to cast {} to {}",
                                            to_string(val),
                                            dest_type,
                                        )
                                    })?
                                };
                                output.push(value);
                                Ok(())
                            },
                        ),
                    );

                let name = format!("try_to_{dest_type}").to_lowercase();
                registry
                    .register_combine_nullable_1_arg::<VariantType, NumberType<NUM_TYPE>, _, _>(
                        &name,
                        FunctionProperty::default(),
                        |_| FunctionDomain::Full,
                        vectorize_with_builder_1_arg::<
                            VariantType,
                            NullableType<NumberType<NUM_TYPE>>,
                        >(move |val, output, _| {
                            if dest_type.is_float() {
                                if let Ok(value) = to_f64(val) {
                                    if let Some(new_value) = num_traits::cast::cast(value) {
                                        output.push(new_value);
                                    } else {
                                        output.push_null();
                                    }
                                } else {
                                    output.push_null();
                                }
                            } else if dest_type.is_signed() {
                                if let Ok(value) = to_i64(val) {
                                    if let Some(new_value) = num_traits::cast::cast(value) {
                                        output.push(new_value);
                                    } else {
                                        output.push_null();
                                    }
                                } else {
                                    output.push_null();
                                }
                            } else {
                                if let Ok(value) = to_u64(val) {
                                    if let Some(new_value) = num_traits::cast::cast(value) {
                                        output.push(new_value);
                                    } else {
                                        output.push_null();
                                    }
                                } else {
                                    output.push_null();
                                }
                            }
                            Ok(())
                        }),
                    );
            }
        });
    }
}

fn register_compare_functions(registry: &mut FunctionRegistry) {
    for name in ALL_COMP_FUNC_NAMES {
        registry.register_auto_cast_signatures(name, vec![
            (DataType::Boolean, DataType::Variant),
            (DataType::Date, DataType::Variant),
            (DataType::Timestamp, DataType::Variant),
            (DataType::String, DataType::Variant),
            (DataType::Number(NumberDataType::UInt8), DataType::Variant),
            (DataType::Number(NumberDataType::UInt16), DataType::Variant),
            (DataType::Number(NumberDataType::UInt32), DataType::Variant),
            (DataType::Number(NumberDataType::UInt64), DataType::Variant),
            (DataType::Number(NumberDataType::Int8), DataType::Variant),
            (DataType::Number(NumberDataType::Int16), DataType::Variant),
            (DataType::Number(NumberDataType::Int32), DataType::Variant),
            (DataType::Number(NumberDataType::Int64), DataType::Variant),
            (DataType::Number(NumberDataType::Float32), DataType::Variant),
            (DataType::Number(NumberDataType::Float64), DataType::Variant),
        ]);
    }
}

fn register_match_functions(registry: &mut FunctionRegistry) {
    for name in ALL_MATCH_FUNC_NAMES {
        registry.register_auto_cast_signatures(name, vec![(DataType::String, DataType::Variant)]);
    }
}
