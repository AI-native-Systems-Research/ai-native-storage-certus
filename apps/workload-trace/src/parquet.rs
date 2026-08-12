//! The parquet container (spec T083, T084), behind the `parquet` feature.
//!
//! Same records as the JSONL container, columnar. `contracts/trace-io.md` is
//! explicit that this is one schema in two containers and that a reader must accept
//! either for any operation, so everything here is about bytes becoming
//! [`Invocation`] values and back — no statistic, no normalisation, no
//! interpretation. Those live one level up, shared, in [`crate::read`].
//!
//! ## Columns are matched by name, never by position
//!
//! The contract says a third-party reader matches on field names, which cuts both
//! ways: a trace written by another tool may order its columns differently, carry
//! extra ones, or omit ones it has no data for. So every column is looked up by
//! name and a missing one is either an error or an empty default depending on
//! whether the record can be understood without it. Reading by index would work on
//! this tool's own output and silently mis-assign every field on someone else's —
//! the same class of bug as the trailing-partial-block trap the contract records,
//! and quieter.
//!
//! ## What is deliberately not here
//!
//! No `blocks/block_size_<N>/` reader. That table maps block id to role, and
//! nothing in the model consumes a role: `role_codes` is carried verbatim in the
//! manifest and consulted by nothing. Reading it would produce a value with no
//! reader, which is the kind of half-built path that later looks like a capability.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanArray, BooleanBuilder, Float64Array, Float64Builder, Int64Array,
    Int64Builder, ListArray, ListBuilder, StringArray, StringBuilder,
};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use workload_model::trace::{Invocation, TraceManifest};

use crate::read::{assemble, read_manifest, resolve_block_size, ReadError, Trace};

/// Rows per record batch, on both read and write.
///
/// A trace is millions of rows and the reader materialises [`Invocation`] values, so
/// the batch size bounds arrow's working set rather than the result's. 8192 is
/// arrow's own default order of magnitude and small enough that a batch's list
/// offsets stay in cache; nothing here is sensitive to the exact value.
const BATCH_ROWS: usize = 8192;

/// The invocation schema of `contracts/trace-io.md`, spelled exactly.
///
/// Nullability follows the contract rather than convenience: `session_id`,
/// `request_start`, `request_end`, `model` and `partial_final_valid` are nullable
/// there and so are nullable here, because a writer that declared them required
/// would be unable to express a real trace that omits them.
pub fn schema() -> SchemaRef {
    let list = |name: &str| Field::new(name, list_of_i64(), false);
    Arc::new(Schema::new(vec![
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, true),
        Field::new("invocation_index", DataType::Int64, false),
        Field::new("parent_invocation", DataType::Int64, false),
        list("parent_invocations"),
        Field::new("request_start", DataType::Float64, true),
        Field::new("request_end", DataType::Float64, true),
        Field::new("timestamp_kind", DataType::Utf8, false),
        Field::new("timestamp_is_synthetic", DataType::Boolean, false),
        Field::new("model", DataType::Utf8, true),
        Field::new("input_length", DataType::Int64, false),
        Field::new("output_length", DataType::Int64, false),
        list("reuse_from"),
        list("new_input_blocks"),
        list("new_output_blocks"),
        list("full_input_blocks"),
        list("full_output_blocks"),
        Field::new("partial_final_valid", DataType::Int64, true),
    ]))
}

/// `list<int64>`, with the item field arrow's own default name so that a file
/// written here is readable by any arrow-based tool without a schema hint.
fn list_of_i64() -> DataType {
    DataType::List(Arc::new(Field::new("item", DataType::Int64, true)))
}

/// Where one blocking's invocations live, per the contract's layout.
pub fn partition_dir(root: &Path, block_size: u32) -> PathBuf {
    root.join("invocations")
        .join(format!("block_size_{block_size}"))
}

/// Write invocations as a single `part-0.parquet` under the contract's layout.
///
/// One part file rather than many: partitioning exists so a reader can select a
/// blocking, which the directory name already does, and a second axis would be a
/// sharding scheme this tool has no basis to choose. The reader accepts any number
/// of parts, so a trace split by another writer still reads.
pub fn write_invocations(
    root: &Path,
    block_size: u32,
    rows: &[Invocation],
) -> Result<PathBuf, ReadError> {
    let dir = partition_dir(root, block_size);
    std::fs::create_dir_all(&dir).map_err(|e| ReadError::Io(format!("{}: {e}", dir.display())))?;
    let path = dir.join("part-0.parquet");
    let file =
        File::create(&path).map_err(|e| ReadError::Io(format!("{}: {e}", path.display())))?;
    let props = WriterProperties::builder().build();
    let mut writer = ArrowWriter::try_new(file, schema(), Some(props))
        .map_err(|e| ReadError::Io(format!("{}: {e}", path.display())))?;
    for chunk in rows.chunks(BATCH_ROWS.max(1)) {
        let batch = to_batch(chunk)?;
        writer
            .write(&batch)
            .map_err(|e| ReadError::Io(format!("{}: {e}", path.display())))?;
    }
    writer
        .close()
        .map_err(|e| ReadError::Io(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// One record batch from a slice of invocations.
fn to_batch(rows: &[Invocation]) -> Result<RecordBatch, ReadError> {
    let mut trace_id = StringBuilder::new();
    let mut session_id = StringBuilder::new();
    let mut invocation_index = Int64Builder::new();
    let mut parent_invocation = Int64Builder::new();
    let mut parent_invocations = ListBuilder::new(Int64Builder::new());
    let mut request_start = Float64Builder::new();
    let mut request_end = Float64Builder::new();
    let mut timestamp_kind = StringBuilder::new();
    let mut timestamp_is_synthetic = BooleanBuilder::new();
    let mut model = StringBuilder::new();
    let mut input_length = Int64Builder::new();
    let mut output_length = Int64Builder::new();
    let mut reuse_from = ListBuilder::new(Int64Builder::new());
    let mut new_input_blocks = ListBuilder::new(Int64Builder::new());
    let mut new_output_blocks = ListBuilder::new(Int64Builder::new());
    let mut full_input_blocks = ListBuilder::new(Int64Builder::new());
    let mut full_output_blocks = ListBuilder::new(Int64Builder::new());
    let mut partial_final_valid = Int64Builder::new();

    fn push_list(b: &mut ListBuilder<Int64Builder>, v: &[i64]) {
        b.values().append_slice(v);
        b.append(true);
    }

    for r in rows {
        trace_id.append_value(&r.trace_id);
        session_id.append_option(r.session_id.as_deref());
        invocation_index.append_value(r.invocation_index);
        parent_invocation.append_value(r.parent_invocation);
        push_list(&mut parent_invocations, &r.parent_invocations);
        request_start.append_option(r.request_start);
        request_end.append_option(r.request_end);
        timestamp_kind.append_value(&r.timestamp_kind);
        timestamp_is_synthetic.append_value(r.timestamp_is_synthetic);
        model.append_option(r.model.as_deref());
        input_length.append_value(r.input_length);
        output_length.append_value(r.output_length);
        push_list(&mut reuse_from, &r.reuse_from);
        push_list(&mut new_input_blocks, &r.new_input_blocks);
        push_list(&mut new_output_blocks, &r.new_output_blocks);
        push_list(&mut full_input_blocks, &r.full_input_blocks);
        push_list(&mut full_output_blocks, &r.full_output_blocks);
        partial_final_valid.append_value(r.partial_final_valid);
    }

    let columns: Vec<ArrayRef> = vec![
        Arc::new(trace_id.finish()),
        Arc::new(session_id.finish()),
        Arc::new(invocation_index.finish()),
        Arc::new(parent_invocation.finish()),
        Arc::new(parent_invocations.finish()),
        Arc::new(request_start.finish()),
        Arc::new(request_end.finish()),
        Arc::new(timestamp_kind.finish()),
        Arc::new(timestamp_is_synthetic.finish()),
        Arc::new(model.finish()),
        Arc::new(input_length.finish()),
        Arc::new(output_length.finish()),
        Arc::new(reuse_from.finish()),
        Arc::new(new_input_blocks.finish()),
        Arc::new(new_output_blocks.finish()),
        Arc::new(full_input_blocks.finish()),
        Arc::new(full_output_blocks.finish()),
        Arc::new(partial_final_valid.finish()),
    ];
    RecordBatch::try_new(schema(), columns).map_err(|e| ReadError::Io(e.to_string()))
}

/// Every `part-*.parquet` of one blocking, in sorted filename order.
///
/// Sorted so a multi-part trace reads in a deterministic order. That matters even
/// though the reader sorts by timestamp afterwards: a trace without timestamps is
/// read in *file* order, and a fit from it is reported as order-dependent (FR-055d)
/// — order-dependent must still mean reproducible.
fn part_files(dir: &Path) -> Result<Vec<PathBuf>, ReadError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ReadError::Io(format!("{}: {e}", dir.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ReadError::Io(format!("{}: {e}", dir.display())))?;
    let mut parts: Vec<PathBuf> = entries
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .collect();
    parts.sort();
    if parts.is_empty() {
        return Err(ReadError::Io(format!(
            "{}: no part-*.parquet files, so this blocking's partition is empty",
            dir.display()
        )));
    }
    Ok(parts)
}

/// Read every invocation of one blocking.
pub fn read_invocations(root: &Path, block_size: u32) -> Result<Vec<Invocation>, ReadError> {
    let dir = partition_dir(root, block_size);
    let mut out = Vec::new();
    for part in part_files(&dir)? {
        let file =
            File::open(&part).map_err(|e| ReadError::Io(format!("{}: {e}", part.display())))?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| ReadError::Io(format!("{}: {e}", part.display())))?
            .with_batch_size(BATCH_ROWS)
            .build()
            .map_err(|e| ReadError::Io(format!("{}: {e}", part.display())))?;
        for batch in reader {
            let batch = batch.map_err(|e| ReadError::Io(format!("{}: {e}", part.display())))?;
            from_batch(&batch, &part, &mut out)?;
        }
    }
    Ok(out)
}

/// A column by name, or `None` if the file does not carry it.
fn column<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a ArrayRef> {
    batch.schema().index_of(name).ok().map(|i| batch.column(i))
}

/// A required column, downcast, or an error naming the file and the column.
macro_rules! required {
    ($batch:expr, $part:expr, $name:literal, $ty:ty) => {
        match column($batch, $name).and_then(|c| c.as_any().downcast_ref::<$ty>()) {
            Some(c) => c,
            None => {
                return Err(ReadError::Io(format!(
                    "{}: column `{}` is missing or is not {}; every invocation record needs it \
                     to be interpretable at all",
                    $part.display(),
                    $name,
                    stringify!($ty)
                )))
            }
        }
    };
}

/// Append one batch's rows to `out`.
fn from_batch(
    batch: &RecordBatch,
    part: &Path,
    out: &mut Vec<Invocation>,
) -> Result<(), ReadError> {
    let trace_id = required!(batch, part, "trace_id", StringArray);
    let invocation_index = required!(batch, part, "invocation_index", Int64Array);
    let input_length = required!(batch, part, "input_length", Int64Array);

    // Optional columns. An absent one is empty or null rather than an error,
    // because the contract admits records that omit what they have no data for —
    // `parent_invocations` explicitly ("a reader MUST treat an absent
    // `parent_invocations` as empty, not as unknown"), and the delta/full split
    // means one of the two block encodings is always the missing one.
    let opt_str =
        |n: &str| column(batch, n).and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned());
    let opt_i64 =
        |n: &str| column(batch, n).and_then(|c| c.as_any().downcast_ref::<Int64Array>().cloned());
    let opt_f64 =
        |n: &str| column(batch, n).and_then(|c| c.as_any().downcast_ref::<Float64Array>().cloned());
    let opt_bool =
        |n: &str| column(batch, n).and_then(|c| c.as_any().downcast_ref::<BooleanArray>().cloned());
    let opt_list =
        |n: &str| column(batch, n).and_then(|c| c.as_any().downcast_ref::<ListArray>().cloned());

    let session_id = opt_str("session_id");
    let timestamp_kind = opt_str("timestamp_kind");
    let model = opt_str("model");
    let parent_invocation = opt_i64("parent_invocation");
    let output_length = opt_i64("output_length");
    let partial_final_valid = opt_i64("partial_final_valid");
    let request_start = opt_f64("request_start");
    let request_end = opt_f64("request_end");
    let timestamp_is_synthetic = opt_bool("timestamp_is_synthetic");
    let parent_invocations = opt_list("parent_invocations");
    let reuse_from = opt_list("reuse_from");
    let new_input_blocks = opt_list("new_input_blocks");
    let new_output_blocks = opt_list("new_output_blocks");
    let full_input_blocks = opt_list("full_input_blocks");
    let full_output_blocks = opt_list("full_output_blocks");

    for i in 0..batch.num_rows() {
        out.push(Invocation {
            trace_id: trace_id.value(i).to_string(),
            session_id: string_at(&session_id, i),
            invocation_index: invocation_index.value(i),
            // −1 is the contract's "no predecessor", so it is also the right answer
            // for a trace that does not carry the column.
            parent_invocation: i64_at(&parent_invocation, i).unwrap_or(-1),
            parent_invocations: list_at(&parent_invocations, i, part)?,
            request_start: f64_at(&request_start, i),
            request_end: f64_at(&request_end, i),
            timestamp_kind: string_at(&timestamp_kind, i).unwrap_or_default(),
            timestamp_is_synthetic: timestamp_is_synthetic
                .as_ref()
                .filter(|a| a.is_valid(i))
                .map(|a| a.value(i))
                .unwrap_or(false),
            model: string_at(&model, i),
            input_length: input_length.value(i),
            output_length: i64_at(&output_length, i).unwrap_or(0),
            reuse_from: list_at(&reuse_from, i, part)?,
            new_input_blocks: list_at(&new_input_blocks, i, part)?,
            new_output_blocks: list_at(&new_output_blocks, i, part)?,
            full_input_blocks: list_at(&full_input_blocks, i, part)?,
            full_output_blocks: list_at(&full_output_blocks, i, part)?,
            partial_final_valid: i64_at(&partial_final_valid, i).unwrap_or(0),
        });
    }
    Ok(())
}

fn string_at(a: &Option<StringArray>, i: usize) -> Option<String> {
    a.as_ref()
        .filter(|a| a.is_valid(i))
        .map(|a| a.value(i).to_string())
}

fn i64_at(a: &Option<Int64Array>, i: usize) -> Option<i64> {
    a.as_ref().filter(|a| a.is_valid(i)).map(|a| a.value(i))
}

fn f64_at(a: &Option<Float64Array>, i: usize) -> Option<f64> {
    a.as_ref().filter(|a| a.is_valid(i)).map(|a| a.value(i))
}

/// One row of a `list<int64>` column, or empty where the column or the row is null.
///
/// A null *list* is empty, per the contract's rule for `parent_invocations`. A null
/// *element* inside a list is an error rather than a skip: block ids are positions
/// in an ordered path, so dropping one would silently shorten the path — the same
/// off-by-one-per-request failure as misreading the trailing partial block.
fn list_at(a: &Option<ListArray>, i: usize, part: &Path) -> Result<Vec<i64>, ReadError> {
    let Some(a) = a.as_ref().filter(|a| a.is_valid(i)) else {
        return Ok(Vec::new());
    };
    let values = a.value(i);
    let ints = values
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| {
            ReadError::Io(format!(
                "{}: a list column's items are {:?}, not int64; block ids are dense integers \
                 in mint order",
                part.display(),
                values.data_type()
            ))
        })?;
    if ints.null_count() > 0 {
        return Err(ReadError::Io(format!(
            "{}: a block list contains a null element. Block ids are positions in an ordered \
             path, so skipping one would silently shorten every path it appears in",
            part.display()
        )));
    }
    Ok(ints.values().to_vec())
}

/// Read a whole parquet trace: manifest, one blocking's invocations, then the
/// shared normalisation both containers use.
pub fn read_trace(
    root: &Path,
    allow_partial: bool,
    block_size: Option<u32>,
) -> Result<Trace, ReadError> {
    let manifest = read_manifest(root)?;
    let block_size = resolve_block_size(&manifest, block_size)?;
    let rows = read_invocations(root, block_size)?;
    assemble(manifest, rows, block_size, allow_partial)
}

/// Write a trace: one blocking's invocations plus the manifest beside them.
pub fn write_trace(
    root: &Path,
    manifest: &TraceManifest,
    block_size: u32,
    rows: &[Invocation],
) -> Result<PathBuf, ReadError> {
    let part = write_invocations(root, block_size, rows)?;
    let manifest_path = root.join("manifest.json");
    let json = manifest
        .to_json()
        .map_err(|e| ReadError::Manifest(e.to_string()))?;
    std::fs::write(&manifest_path, json)
        .map_err(|e| ReadError::Io(format!("{}: {e}", manifest_path.display())))?;
    Ok(part)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(session: &str, index: i64, blocks: &[i64]) -> Invocation {
        Invocation {
            trace_id: "t".into(),
            session_id: Some(session.into()),
            invocation_index: index,
            parent_invocation: index - 1,
            parent_invocations: vec![],
            request_start: Some(index as f64),
            request_end: None,
            timestamp_kind: "start".into(),
            timestamp_is_synthetic: true,
            model: None,
            input_length: blocks.len() as i64 * 16,
            output_length: 0,
            reuse_from: vec![],
            new_input_blocks: vec![],
            new_output_blocks: vec![],
            full_input_blocks: blocks.to_vec(),
            full_output_blocks: vec![],
            partial_final_valid: 0,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("certus-parquet-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_record_survives_the_container_exactly() {
        // The claim `contracts/trace-io.md` makes is that the container is not
        // information. That is only testable as equality on the record itself —
        // every field, including the nullable ones, which are where a container
        // conversion loses things.
        let dir = tmp("exact");
        let rows = vec![
            row("s0", 0, &[1, 2, 3]),
            row("s0", 1, &[1, 2, 3, 4, 5]),
            row("s1", 0, &[1, 9]),
        ];
        write_invocations(&dir, 16, &rows).expect("write");
        let back = read_invocations(&dir, 16).expect("read");
        assert_eq!(back, rows, "the container changed the records");
    }

    #[test]
    fn nulls_and_empty_lists_are_distinguished_from_missing_ones() {
        // `session_id`, `request_end` and `model` are nullable in the contract, and a
        // writer that silently substituted "" or 0.0 would make a trace that omits
        // them indistinguishable from one that has those values.
        let dir = tmp("nulls");
        let mut r = row("s", 0, &[7]);
        r.session_id = None;
        r.request_start = None;
        r.model = None;
        write_invocations(&dir, 16, &[r.clone()]).expect("write");
        let back = read_invocations(&dir, 16).expect("read");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].session_id, None, "an absent session became a value");
        assert_eq!(back[0].request_start, None);
        assert_eq!(back[0].model, None);
        assert!(back[0].reuse_from.is_empty());
        assert_eq!(back[0].full_input_blocks, vec![7]);
    }

    #[test]
    fn an_empty_partition_is_refused_rather_than_read_as_an_empty_trace() {
        // The failure mode this guards: a mistyped block size names a directory that
        // does not exist, or exists empty, and a reader returning zero rows would
        // report a trace with no requests instead of a path that is wrong.
        let dir = tmp("empty");
        std::fs::create_dir_all(partition_dir(&dir, 16)).unwrap();
        let e = read_invocations(&dir, 16).expect_err("must refuse");
        assert!(e.to_string().contains("no part-*.parquet"), "{e}");
    }

    #[test]
    fn columns_are_found_by_name_not_by_position() {
        // A third-party writer may order columns differently. Reading by index would
        // pass on our own output and mis-assign every field on theirs, so the test
        // writes a deliberately reordered file and expects the values to land in the
        // right fields.
        let dir = tmp("reordered");
        let reordered = Arc::new(Schema::new(vec![
            Field::new("input_length", DataType::Int64, false),
            Field::new("full_input_blocks", list_of_i64(), false),
            Field::new("trace_id", DataType::Utf8, false),
            Field::new("invocation_index", DataType::Int64, false),
        ]));
        let mut blocks = ListBuilder::new(Int64Builder::new());
        blocks.values().append_slice(&[4, 5, 6]);
        blocks.append(true);
        let batch = RecordBatch::try_new(
            reordered.clone(),
            vec![
                Arc::new(Int64Array::from(vec![48])),
                Arc::new(blocks.finish()),
                Arc::new(StringArray::from(vec!["t"])),
                Arc::new(Int64Array::from(vec![0])),
            ],
        )
        .unwrap();
        let part_dir = partition_dir(&dir, 16);
        std::fs::create_dir_all(&part_dir).unwrap();
        let f = File::create(part_dir.join("part-0.parquet")).unwrap();
        let mut w = ArrowWriter::try_new(f, reordered, None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let back = read_invocations(&dir, 16).expect("read");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].trace_id, "t");
        assert_eq!(back[0].input_length, 48);
        assert_eq!(back[0].full_input_blocks, vec![4, 5, 6]);
        // Columns the file does not carry take the contract's own defaults, not an
        // error: a trace omits what it has no data for.
        assert_eq!(back[0].parent_invocation, -1, "absent means no predecessor");
        assert!(back[0].parent_invocations.is_empty());
        assert_eq!(back[0].request_start, None);
    }

    #[test]
    fn a_missing_required_column_names_itself() {
        // `input_length` is what both encodings' length invariants are checked
        // against, so a file without it cannot be interpreted — and saying which
        // column is missing is the difference between a fixable error and a puzzle.
        let dir = tmp("missing");
        let thin = Arc::new(Schema::new(vec![
            Field::new("trace_id", DataType::Utf8, false),
            Field::new("invocation_index", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            thin.clone(),
            vec![
                Arc::new(StringArray::from(vec!["t"])),
                Arc::new(Int64Array::from(vec![0])),
            ],
        )
        .unwrap();
        let part_dir = partition_dir(&dir, 16);
        std::fs::create_dir_all(&part_dir).unwrap();
        let f = File::create(part_dir.join("part-0.parquet")).unwrap();
        let mut w = ArrowWriter::try_new(f, thin, None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let e = read_invocations(&dir, 16).expect_err("must refuse");
        assert!(e.to_string().contains("input_length"), "{e}");
    }

    #[test]
    fn a_multi_part_partition_reads_in_a_deterministic_order() {
        // A trace without timestamps is read in file order and its fit reported as
        // order-dependent (FR-055d). Order-dependent still has to be reproducible,
        // so the parts are sorted rather than taken as the directory hands them over.
        let dir = tmp("parts");
        let part_dir = partition_dir(&dir, 16);
        std::fs::create_dir_all(&part_dir).unwrap();
        for (name, idx) in [
            ("part-2.parquet", 2i64),
            ("part-0.parquet", 0),
            ("part-1.parquet", 1),
        ] {
            let batch = to_batch(&[row("s", idx, &[idx + 1])]).unwrap();
            let f = File::create(part_dir.join(name)).unwrap();
            let mut w = ArrowWriter::try_new(f, schema(), None).unwrap();
            w.write(&batch).unwrap();
            w.close().unwrap();
        }
        let back = read_invocations(&dir, 16).expect("read");
        let order: Vec<i64> = back.iter().map(|r| r.invocation_index).collect();
        assert_eq!(order, vec![0, 1, 2], "parts were not read in name order");
    }
}
