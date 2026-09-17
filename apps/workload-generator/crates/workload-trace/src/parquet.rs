//! The parquet container, behind the `parquet` feature.
//!
//! Records identical to the JSONL writer's (T055 asserts it), in the columnar
//! container the corpus's own traces use. Worth the dependency for one reason: the
//! block-list columns repeat the whole prefix on every row, which is the case
//! columnar compression is built for — a JSONL trace of a long-session workload is
//! mostly the same integers written again and again.
//!
//! # Columnar means a row group, so this one buffers
//!
//! The JSONL writer streams: a row is complete when its turn is, and nothing is
//! held. Parquet cannot do that, because a column has to be assembled before it can
//! be encoded. So this writer buffers [`ROW_GROUP_ROWS`] rows and flushes a row
//! group.
//!
//! That is the **one place** the emit path's bounded-memory property is weakened,
//! and it is weakened by a constant rather than by the run: peak buffered rows is
//! the row-group size regardless of span. Recorded here rather than left for someone
//! to discover from a memory graph, and it is why the row-group size is a named
//! constant with the arithmetic beside it rather than a literal.
//!
//! # Nullable columns are nullable on purpose
//!
//! `request_end`, `model` and `partial_final_valid` are all-null columns rather than
//! absent ones, for the reason `record.rs` gives: a reader must learn what a trace
//! supports by reading it (FR-056), and an absent column would force it to guess.
//! Parquet stores an all-null column in a handful of bytes, so this costs nothing.
//!
//! # Examples
//!
//! The writer is driven exactly like the JSONL one — same records, same counts, a
//! different container. That is what makes the two interchangeable for a
//! reproducibility check:
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::sim::Simulation;
//! use workload_trace::parquet::ParquetWriter;
//!
//! let description: WorkloadDescription = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   docs: {length: {constant: 3}, lifetime: {constant: .inf}, pool: {size: {exact: 2}}}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 2}}
//!     uses: [{class: docs, count: {constant: 1}}]
//!     turns: {constant: 3}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 5}
//! "#
//! .parse()
//! .unwrap();
//!
//! let mut bytes = Vec::new();
//! let mut sim = Simulation::new(&description, 7).unwrap();
//! let mut writer = ParquetWriter::new(&mut bytes, "demo", description.blocks.tokens).unwrap();
//! sim.run_until(60.0, &mut |s, t| writer.write(s, t).unwrap());
//! let stats = writer.finish().unwrap();
//!
//! // The same counts the JSONL writer's example reports, from the same seed.
//! assert_eq!(stats.sessions, 9);
//! assert_eq!(stats.invocations, 24);
//! assert_eq!(stats.unique_blocks, 78);
//!
//! // A parquet file, magic bytes at both ends.
//! assert_eq!(&bytes[..4], b"PAR1");
//! assert_eq!(&bytes[bytes.len() - 4..], b"PAR1");
//! ```

use std::io::Write;

use arrow_array::builder::{
    BooleanBuilder, Float64Builder, Int64Builder, ListBuilder, StringBuilder, UInt64Builder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use std::sync::Arc;

use workload_model::session::{Session, Turn};

use crate::manifest::BlockStats;
use crate::record::InvocationRecord;

/// Rows buffered before a row group is flushed.
///
/// At the shipped example's shape a row's block lists average a few hundred
/// integers, so 8 192 rows is on the order of tens of megabytes buffered — large
/// enough for compression to work on a column, small enough to be a constant rather
/// than a function of the run.
pub const ROW_GROUP_ROWS: usize = 8_192;

/// The arrow schema, which is also the column order in the file.
///
/// Deliberately the same field order as [`InvocationRecord`]'s serialisation, so a
/// reader that has seen one container recognises the other.
pub fn schema() -> SchemaRef {
    let u64_list = |name: &str| {
        Field::new(
            name,
            DataType::List(Arc::new(Field::new("item", DataType::UInt64, false))),
            false,
        )
    };
    Arc::new(Schema::new(vec![
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("invocation_index", DataType::Int64, false),
        Field::new("parent_invocation", DataType::Int64, false),
        Field::new("request_start", DataType::Float64, false),
        // Nullable and always null; see the module docs.
        Field::new("request_end", DataType::Float64, true),
        Field::new("timestamp_kind", DataType::Utf8, false),
        Field::new("timestamp_is_synthetic", DataType::Boolean, false),
        Field::new("model", DataType::Utf8, true),
        Field::new("input_length", DataType::Int64, false),
        Field::new("output_length", DataType::Int64, false),
        Field::new(
            "reuse_from",
            DataType::List(Arc::new(Field::new("item", DataType::Int64, false))),
            false,
        ),
        u64_list("new_input_blocks"),
        u64_list("new_output_blocks"),
        u64_list("full_input_blocks"),
        u64_list("full_output_blocks"),
        Field::new("partial_final_valid", DataType::Int64, true),
    ]))
}

/// Writes invocation records as parquet.
pub struct ParquetWriter<W: Write + Send> {
    writer: ArrowWriter<W>,
    trace_id: String,
    block_size: u64,
    pending: Vec<InvocationRecord>,
    sessions: std::collections::BTreeSet<u64>,
    distinct_keys: std::collections::BTreeSet<u64>,
    invocations: u64,
    previous: std::collections::HashMap<String, InvocationRecord>,
}

impl<W: Write + Send> ParquetWriter<W> {
    /// A writer for one container file.
    ///
    /// Compressed with zstd: the block-list columns are highly repetitive, and the
    /// projection's byte figure is an *uncompressed* upper bound precisely because
    /// the achieved ratio cannot be predicted (FR-073).
    ///
    /// # Errors
    ///
    /// If the parquet writer cannot be created.
    pub fn new(sink: W, trace_id: &str, block_size: u64) -> parquet::errors::Result<Self> {
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build();
        Ok(Self {
            writer: ArrowWriter::try_new(sink, schema(), Some(props))?,
            trace_id: trace_id.to_string(),
            block_size,
            pending: Vec::with_capacity(ROW_GROUP_ROWS),
            sessions: Default::default(),
            distinct_keys: Default::default(),
            invocations: 0,
            previous: Default::default(),
        })
    }

    /// Write one turn's record.
    ///
    /// Row verification is on, as in the JSONL writer: FR-058 is a property of every
    /// emitted row, not of a container.
    ///
    /// # Errors
    ///
    /// If a row violates the schema's invariants, or the parquet writer fails.
    pub fn write(&mut self, session: &Session, turn: &Turn) -> parquet::errors::Result<()> {
        let record = InvocationRecord::from_turn(&self.trace_id, session, turn, self.block_size);
        if let Err(e) = record.check(self.block_size, self.previous.get(&record.session_id)) {
            return Err(parquet::errors::ParquetError::General(format!(
                "emitted row violates the trace schema: {e}"
            )));
        }
        self.sessions.insert(session.id());
        self.distinct_keys.extend(&record.full_input_blocks);
        self.distinct_keys.extend(&record.full_output_blocks);
        self.invocations += 1;
        self.previous
            .insert(record.session_id.clone(), record.clone());
        self.pending.push(record);
        if self.pending.len() >= ROW_GROUP_ROWS {
            self.flush_group()?;
        }
        Ok(())
    }

    /// Flush the last row group, close the file, and return the manifest counts.
    ///
    /// # Errors
    ///
    /// If the final flush or close fails.
    pub fn finish(mut self) -> parquet::errors::Result<BlockStats> {
        if !self.pending.is_empty() {
            self.flush_group()?;
        }
        self.writer.close()?;
        Ok(BlockStats {
            sessions: self.sessions.len() as u64,
            invocations: self.invocations,
            unique_blocks: self.distinct_keys.len() as u64,
        })
    }

    fn flush_group(&mut self) -> parquet::errors::Result<()> {
        let batch = to_batch(&self.pending)?;
        self.writer.write(&batch)?;
        self.pending.clear();
        Ok(())
    }
}

/// Build one record batch from a slice of records.
///
/// # Errors
///
/// If arrow rejects the arrays, which would mean this module and [`schema`] have
/// drifted apart.
pub fn to_batch(rows: &[InvocationRecord]) -> parquet::errors::Result<RecordBatch> {
    let mut trace_id = StringBuilder::new();
    let mut session_id = StringBuilder::new();
    let mut invocation_index = Int64Builder::new();
    let mut parent_invocation = Int64Builder::new();
    let mut request_start = Float64Builder::new();
    let mut request_end = Float64Builder::new();
    let mut timestamp_kind = StringBuilder::new();
    let mut timestamp_is_synthetic = BooleanBuilder::new();
    let mut model = StringBuilder::new();
    let mut input_length = Int64Builder::new();
    let mut output_length = Int64Builder::new();
    // `ListBuilder` marks its item field nullable by default. The schema says these
    // items are never null, which is true of the data, so the builder is told rather
    // than the schema weakened — a trace's schema is part of its self-description
    // (FR-056), and "possibly null" would be a claim about the data that is false.
    let item = |ty: DataType| Arc::new(Field::new("item", ty, false));
    let mut reuse_from = ListBuilder::new(Int64Builder::new()).with_field(item(DataType::Int64));
    let mut new_input = ListBuilder::new(UInt64Builder::new()).with_field(item(DataType::UInt64));
    let mut new_output = ListBuilder::new(UInt64Builder::new()).with_field(item(DataType::UInt64));
    let mut full_input = ListBuilder::new(UInt64Builder::new()).with_field(item(DataType::UInt64));
    let mut full_output = ListBuilder::new(UInt64Builder::new()).with_field(item(DataType::UInt64));
    let mut partial_final_valid = Int64Builder::new();

    for r in rows {
        trace_id.append_value(&r.trace_id);
        session_id.append_value(&r.session_id);
        invocation_index.append_value(r.invocation_index);
        parent_invocation.append_value(r.parent_invocation);
        request_start.append_value(r.request_start);
        request_end.append_option(r.request_end);
        timestamp_kind.append_value(r.timestamp_kind);
        timestamp_is_synthetic.append_value(r.timestamp_is_synthetic);
        model.append_option(r.model.as_deref());
        input_length.append_value(r.input_length);
        output_length.append_value(r.output_length);
        for v in &r.reuse_from {
            reuse_from.values().append_value(*v);
        }
        reuse_from.append(true);
        for (builder, keys) in [
            (&mut new_input, &r.new_input_blocks),
            (&mut new_output, &r.new_output_blocks),
            (&mut full_input, &r.full_input_blocks),
            (&mut full_output, &r.full_output_blocks),
        ] {
            for k in keys.iter() {
                builder.values().append_value(*k);
            }
            builder.append(true);
        }
        partial_final_valid.append_option(r.partial_final_valid);
    }

    let columns: Vec<ArrayRef> = vec![
        Arc::new(trace_id.finish()),
        Arc::new(session_id.finish()),
        Arc::new(invocation_index.finish()),
        Arc::new(parent_invocation.finish()),
        Arc::new(request_start.finish()),
        Arc::new(request_end.finish()),
        Arc::new(timestamp_kind.finish()),
        Arc::new(timestamp_is_synthetic.finish()),
        Arc::new(model.finish()),
        Arc::new(input_length.finish()),
        Arc::new(output_length.finish()),
        Arc::new(reuse_from.finish()),
        Arc::new(new_input.finish()),
        Arc::new(new_output.finish()),
        Arc::new(full_input.finish()),
        Arc::new(full_output.finish()),
        Arc::new(partial_final_valid.finish()),
    ];
    RecordBatch::try_new(schema(), columns)
        .map_err(|e| parquet::errors::ParquetError::General(e.to_string()))
}
