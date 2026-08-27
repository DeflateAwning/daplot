//! Loading CSV / Parquet files into a small in-memory columnar `Table`,
//! with automatic per-column type inference (numeric / datetime / text).

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug)]
pub enum ColumnKind {
    /// Plain numeric values. NaN marks a missing / unparsable value.
    Numeric(Vec<f64>),
    /// Timestamps stored as unix seconds (UTC, may be fractional). NaN = missing.
    DateTime(Vec<f64>),
    /// Free text / categorical values.
    Text(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct ColumnData {
    pub name: String,
    pub kind: ColumnKind,
}

impl ColumnData {
    pub fn type_label(&self) -> &'static str {
        match self.kind {
            ColumnKind::Numeric(_) => "number",
            ColumnKind::DateTime(_) => "datetime",
            ColumnKind::Text(_) => "text",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub columns: Vec<ColumnData>,
    pub row_count: usize,
}

impl Table {
    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    pub fn is_datetime(&self, idx: usize) -> bool {
        matches!(self.columns[idx].kind, ColumnKind::DateTime(_))
    }

    #[allow(dead_code)]
    pub fn is_numeric(&self, idx: usize) -> bool {
        matches!(self.columns[idx].kind, ColumnKind::Numeric(_))
    }

    /// All column names whose kind is Numeric or DateTime (sensible choices for a Y axis).
    pub fn plottable_y_columns(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|c| !matches!(c.kind, ColumnKind::Text(_)))
            .map(|c| c.name.clone())
            .collect()
    }

    /// First column detected as a datetime column, if any.
    pub fn first_datetime_column(&self) -> Option<String> {
        self.columns
            .iter()
            .find(|c| matches!(c.kind, ColumnKind::DateTime(_)))
            .map(|c| c.name.clone())
    }

    /// Return the column's values as f64. Text columns are mapped to a stable
    /// per-value category index (0, 1, 2, ... in order of first appearance).
    pub fn as_f64(&self, idx: usize) -> Vec<f64> {
        match &self.columns[idx].kind {
            ColumnKind::Numeric(v) => v.clone(),
            ColumnKind::DateTime(v) => v.clone(),
            ColumnKind::Text(v) => {
                let mut map: HashMap<&str, usize> = HashMap::new();
                let mut out = Vec::with_capacity(v.len());
                for s in v {
                    let len = map.len();
                    let idx = *map.entry(s.as_str()).or_insert(len);
                    out.push(idx as f64);
                }
                out
            }
        }
    }

    /// For a text column, the unique labels in order of first appearance
    /// (index in this vector matches the category index produced by `as_f64`).
    pub fn text_labels(&self, idx: usize) -> Option<Vec<String>> {
        if let ColumnKind::Text(v) = &self.columns[idx].kind {
            let mut seen = HashSet::new();
            let mut labels = Vec::new();
            for s in v {
                if seen.insert(s.clone()) {
                    labels.push(s.clone());
                }
            }
            Some(labels)
        } else {
            None
        }
    }

    pub fn load(path: &Path) -> Result<Table> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "csv" | "tsv" | "txt" => load_csv(path, if ext == "tsv" { b'\t' } else { b',' }),
            "parquet" | "pq" => load_parquet(path),
            _ => bail!("Unsupported file extension: .{ext} (expected .csv, .tsv or .parquet)"),
        }
    }
}

// ---------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------

fn load_csv(path: &Path, delimiter: u8) -> Result<Table> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(delimiter)
        .from_path(path)
        .with_context(|| format!("opening {:?}", path))?;

    let headers: Vec<String> = rdr
        .headers()
        .context("reading CSV header row")?
        .iter()
        .map(|s| s.trim().to_string())
        .collect();

    let mut raw: Vec<Vec<String>> = vec![Vec::new(); headers.len()];
    for result in rdr.records() {
        let record = result.context("reading CSV row")?;
        for (i, col) in raw.iter_mut().enumerate() {
            col.push(record.get(i).unwrap_or("").to_string());
        }
    }

    let row_count = raw.first().map(|c| c.len()).unwrap_or(0);
    let mut columns = Vec::with_capacity(headers.len());
    for (name, values) in headers.into_iter().zip(raw) {
        let kind = infer_column(&values);
        columns.push(ColumnData { name, kind });
    }

    Ok(Table { columns, row_count })
}

fn infer_column(values: &[String]) -> ColumnKind {
    let non_empty: Vec<&String> = values.iter().filter(|s| !s.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return ColumnKind::Text(values.to_vec());
    }

    if non_empty.iter().all(|s| s.trim().parse::<f64>().is_ok()) {
        let v = values
            .iter()
            .map(|s| s.trim().parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        return ColumnKind::Numeric(v);
    }

    if non_empty.iter().all(|s| parse_datetime(s.trim()).is_some()) {
        let v = values
            .iter()
            .map(|s| {
                if s.trim().is_empty() {
                    f64::NAN
                } else {
                    parse_datetime(s.trim()).unwrap_or(f64::NAN)
                }
            })
            .collect();
        return ColumnKind::DateTime(v);
    }

    ColumnKind::Text(values.to_vec())
}

/// Try a handful of common date/time formats and return unix seconds (UTC).
fn parse_datetime(s: &str) -> Option<f64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }

    const DATETIME_FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M",
        "%d/%m/%Y %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
    ];
    for f in DATETIME_FORMATS {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, f) {
            return Some(Utc.from_utc_datetime(&dt).timestamp() as f64);
        }
    }

    const DATE_FORMATS: &[&str] = &["%Y-%m-%d", "%m/%d/%Y", "%d/%m/%Y", "%Y/%m/%d"];
    for f in DATE_FORMATS {
        if let Ok(d) = NaiveDate::parse_from_str(s, f)
            && let Some(dt) = d.and_hms_opt(0, 0, 0)
        {
            return Some(Utc.from_utc_datetime(&dt).timestamp() as f64);
        }
    }
    None
}

// ---------------------------------------------------------------------
// Parquet (via arrow)
// ---------------------------------------------------------------------

fn load_parquet(path: &Path) -> Result<Table> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    let file = std::fs::File::open(path).with_context(|| format!("opening {:?}", path))?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).context("reading parquet metadata")?;
    let schema = builder.schema().clone();
    let reader = builder.build().context("building parquet reader")?;

    let field_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    let n = field_names.len();

    // 0 = unknown/unset, 1 = numeric, 2 = datetime, 3 = text
    let mut col_type: Vec<u8> = vec![0; n];
    let mut numeric_acc: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut dt_acc: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut text_acc: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut row_count = 0usize;

    for batch_res in reader {
        let batch = batch_res.context("reading parquet batch")?;
        row_count += batch.num_rows();
        for (i, col) in batch.columns().iter().enumerate() {
            let classified = classify_arrow_type(col.data_type());
            if col_type[i] == 0 {
                col_type[i] = classified;
            }
            match col_type[i] {
                1 => append_numeric(col, &mut numeric_acc[i]),
                2 => append_datetime(col, &mut dt_acc[i]),
                _ => append_text(col, &mut text_acc[i]),
            }
        }
    }

    let mut columns = Vec::with_capacity(n);
    for (i, name) in field_names.into_iter().enumerate() {
        let kind = match col_type[i] {
            1 => ColumnKind::Numeric(std::mem::take(&mut numeric_acc[i])),
            2 => ColumnKind::DateTime(std::mem::take(&mut dt_acc[i])),
            _ => ColumnKind::Text(std::mem::take(&mut text_acc[i])),
        };
        columns.push(ColumnData { name, kind });
    }

    Ok(Table { columns, row_count })
}

fn classify_arrow_type(dt: &arrow::datatypes::DataType) -> u8 {
    use arrow::datatypes::DataType::*;
    match dt {
        Int8
        | Int16
        | Int32
        | Int64
        | UInt8
        | UInt16
        | UInt32
        | UInt64
        | Float16
        | Float32
        | Float64
        | Boolean
        | Decimal128(_, _)
        | Decimal256(_, _) => 1,
        Date32 | Date64 | Timestamp(_, _) => 2,
        _ => 3,
    }
}

fn append_numeric(col: &arrow::array::ArrayRef, out: &mut Vec<f64>) {
    use arrow::array::{Array, Float64Array};
    use arrow::compute::cast;
    use arrow::datatypes::DataType;

    if let Ok(casted) = cast(col, &DataType::Float64)
        && let Some(arr) = casted.as_any().downcast_ref::<Float64Array>()
    {
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                f64::NAN
            } else {
                arr.value(i)
            });
        }
        return;
    }
    for _ in 0..col.len() {
        out.push(f64::NAN);
    }
}

fn append_datetime(col: &arrow::array::ArrayRef, out: &mut Vec<f64>) {
    use arrow::array::*;
    use arrow::datatypes::{DataType, TimeUnit};

    match col.data_type() {
        DataType::Date32 => {
            if let Some(arr) = col.as_any().downcast_ref::<Date32Array>() {
                for i in 0..arr.len() {
                    out.push(if arr.is_null(i) {
                        f64::NAN
                    } else {
                        arr.value(i) as f64 * 86_400.0
                    });
                }
                return;
            }
        }
        DataType::Date64 => {
            if let Some(arr) = col.as_any().downcast_ref::<Date64Array>() {
                for i in 0..arr.len() {
                    out.push(if arr.is_null(i) {
                        f64::NAN
                    } else {
                        arr.value(i) as f64 / 1_000.0
                    });
                }
                return;
            }
        }
        DataType::Timestamp(unit, _) => match unit {
            TimeUnit::Second => {
                if let Some(arr) = col.as_any().downcast_ref::<TimestampSecondArray>() {
                    for i in 0..arr.len() {
                        out.push(if arr.is_null(i) {
                            f64::NAN
                        } else {
                            arr.value(i) as f64
                        });
                    }
                    return;
                }
            }
            TimeUnit::Millisecond => {
                if let Some(arr) = col.as_any().downcast_ref::<TimestampMillisecondArray>() {
                    for i in 0..arr.len() {
                        out.push(if arr.is_null(i) {
                            f64::NAN
                        } else {
                            arr.value(i) as f64 / 1e3
                        });
                    }
                    return;
                }
            }
            TimeUnit::Microsecond => {
                if let Some(arr) = col.as_any().downcast_ref::<TimestampMicrosecondArray>() {
                    for i in 0..arr.len() {
                        out.push(if arr.is_null(i) {
                            f64::NAN
                        } else {
                            arr.value(i) as f64 / 1e6
                        });
                    }
                    return;
                }
            }
            TimeUnit::Nanosecond => {
                if let Some(arr) = col.as_any().downcast_ref::<TimestampNanosecondArray>() {
                    for i in 0..arr.len() {
                        out.push(if arr.is_null(i) {
                            f64::NAN
                        } else {
                            arr.value(i) as f64 / 1e9
                        });
                    }
                    return;
                }
            }
        },
        _ => {}
    }
    for _ in 0..col.len() {
        out.push(f64::NAN);
    }
}

fn append_text(col: &arrow::array::ArrayRef, out: &mut Vec<String>) {
    use arrow::array::*;
    use arrow::compute::cast;
    use arrow::datatypes::DataType;

    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                String::new()
            } else {
                arr.value(i).to_string()
            });
        }
        return;
    }
    if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                String::new()
            } else {
                arr.value(i).to_string()
            });
        }
        return;
    }
    if let Ok(casted) = cast(col, &DataType::Utf8)
        && let Some(arr) = casted.as_any().downcast_ref::<StringArray>()
    {
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                String::new()
            } else {
                arr.value(i).to_string()
            });
        }
        return;
    }
    for _ in 0..col.len() {
        out.push(String::new());
    }
}
