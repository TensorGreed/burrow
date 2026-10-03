// qpdf's half of the worker's ambient declarations: the engine, and nothing about which
// bundle it is in.
//
// TWO BUNDLES LINK QPDF SINCE #137, the base one and redaction's, so this file holds only what
// both see -- the factory, the C API and the bridge helpers. What each bundle fetches and what
// its Rust module exports are in `globals-documents.d.ts` and `globals-redact.d.ts`.
//
// Included by `tsconfig.json` and `tsconfig.redact.json`, and NOT by `tsconfig.render.json`,
// which is the point: each project sees exactly the globals its bundle creates. A declaration that outlives the module
// it describes type-checks and then fails at run time in the browser and nowhere else —
// measured, when five `_FPDF_*` declarations stood in `globals.d.ts` for one build after spike
// 0004 stopped loading PDFium. ADR 0026 splits the bundles; this splits what checks them.
//
// Everything here MERGES into the interfaces `globals.d.ts` declares.

/** qpdf's `MODULARIZE`'d factory: the glue exports this rather than filling in a global. */
declare function createQpdfModule(options: object): Promise<EmscriptenModule>;

interface EmscriptenModule {
  // --- the C stack, for an out-parameter that lives for one call. ---
  //
  // THE QPDF MODULE'S, not both modules'. See `globals.d.ts` for what putting them in the
  // shared interface cost.
  stackSave(): number;
  stackAlloc(size: number): number;
  stackRestore(saved: number): void;

  // Exactly the C API `core/burrow-engines/src/qpdf/ffi.rs` declares, which is the set
  // ADR 0013 verified routes through qpdf's `trap_errors`.
  _qpdf_init(): number;
  _qpdf_cleanup(dataPtr: number): void;
  _qpdf_silence_errors(data: number): void;
  _qpdf_set_suppress_warnings(data: number, value: number): void;
  _qpdf_set_logger(data: number, logger: number): void;
  _qpdf_set_attempt_recovery(data: number, value: number): void;
  _qpdf_read_memory(
    data: number,
    description: number,
    buffer: number,
    size: bigint,
    password: number,
  ): number;
  _qpdf_has_error(data: number): number;
  // Whether qpdf raised any warning -- a repair. Only whether, never its code or text (#224).
  _qpdf_more_warnings(data: number): number;
  _qpdf_get_error(data: number): number;
  _qpdf_get_error_code(data: number, error: number): number;
  _qpdf_get_num_pages(data: number): number;
  // The write path, M1 PR B2. Every one matches an entry in `build-wasm.sh`'s
  // EXPORTED_FUNCTIONS allowlist -- a symbol not on that list is not in the module at all.
  _qpdf_get_page_n(data: number, n: number): number;
  // The object at exactly this number and generation; not resolved until read (#227).
  _qpdf_get_object_by_id(data: number, number: number, generation: number): number;
  // The trailer qpdf holds and will write (#239).
  _qpdf_get_trailer(data: number): number;
  _qpdf_add_page(data: number, source: number, page: number, first: number): number;
  _qpdf_remove_page(data: number, page: number): number;
  _qpdf_add_page_at(
    data: number,
    source: number,
    page: number,
    before: number,
    refpage: number,
  ): number;
  _qpdf_init_write_memory(data: number): number;
  /** Note the upstream spelling: `ID` is capitalised in qpdf's C API. */
  _qpdf_set_deterministic_ID(data: number, value: number): void;
  /** The one compression lever. 1 is preserve, 2 is generate (`Constants.h:134-138`). */
  _qpdf_set_object_stream_mode(data: number, mode: number): void;
  _qpdf_write(data: number): number;
  _qpdf_get_buffer_length(data: number): number;
  _qpdf_get_buffer(data: number): number;
  // The object-handle API, added for `rotate`. `key` is a POINTER to a NUL-terminated
  // string in the module's heap, not a JS string: Rust copies the key in and passes the
  // address, so nothing on this side builds or chooses a key.
  _qpdf_oh_get_key(data: number, oh: number, key: number): number;
  _qpdf_oh_get_type_code(data: number, oh: number): number;
  /** `long long` on the C side; the module is built -sWASM_BIGINT=1, so this is a BigInt. */
  _qpdf_oh_get_int_value(data: number, oh: number): bigint;
  _qpdf_oh_new_integer(data: number, value: bigint): number;
  _qpdf_oh_replace_key(data: number, oh: number, key: number, item: number): void;
  _qpdf_oh_get_object_id(data: number, oh: number): number;
  _qpdf_oh_get_generation(data: number, oh: number): number;
  _qpdf_oh_unparse_resolved(data: number, oh: number): number;
  _qpdf_oh_get_name(data: number, oh: number): number;
  _qpdf_oh_remove_key(data: number, oh: number, key: number): void;
  _qpdf_oh_get_array_n_items(data: number, oh: number): number;
  _qpdf_oh_get_array_item(data: number, oh: number, at: number): number;
  _qpdf_oh_erase_item(data: number, oh: number, at: number): void;
  _qpdf_oh_set_array_item(data: number, oh: number, at: number, item: number): void;
  _qpdf_oh_get_dict(data: number, oh: number): number;
  _qpdf_oh_new_null(data: number): number;
  _qpdf_oh_replace_stream_data(
    data: number,
    stream: number,
    buf: number,
    len: number,
    filter: number,
    decodeParms: number,
  ): void;
  /** `bufp` and `lenp` are POINTERS to scratch words the caller owns and must free. */
  _qpdf_oh_get_page_content_data(data: number, page: number, bufp: number, lenp: number): number;
  _qpdf_oh_get_stream_data(
    data: number,
    oh: number,
    level: number,
    filteredp: number,
    bufp: number,
    lenp: number,
  ): number;
  /** Frees a buffer qpdf `malloc`ed and handed to the caller -- the only such buffers here. */
  _qpdf_oh_free_buffer(bufp: number): void;
  _qpdf_oh_release(data: number, oh: number): void;
  _qpdf_global_set_uint32(param: number, value: number): number;
  _qpdflogger_create(): number;
  _qpdflogger_set_info(logger: number, dest: number, a: number, b: number): void;
  _qpdflogger_set_warn(logger: number, dest: number, a: number, b: number): void;
  _qpdflogger_set_error(logger: number, dest: number, a: number, b: number): void;
}

interface WorkerGlobalScope {
  // MIRRORS `QpdfBridge` in `core/burrow-engines/src/web/bridge.rs`, which is the audit
  // surface ADR 0009 relies on: every capability the binding has is one entry there and one
  // entry here. A name in one and not the other is a mismatch worth noticing.
  //
  // Declaring the signatures also contextually types the arrow functions in `bridge-qpdf.js`,
  // so the file that computes heap offsets is checked rather than inferred as `any`.
  __burrow_qpdf_copy_in(bytes: Uint8Array): number;
  __burrow_qpdf_free(ptr: number): void;
  __burrow_qpdf_wipe_free(ptr: number, len: number): void;
  __burrow_qpdf_init(): number;
  __burrow_qpdf_cleanup(data: number): void;
  __burrow_qpdf_silence_errors(data: number): void;
  __burrow_qpdf_set_suppress_warnings(data: number, value: number): void;
  __burrow_qpdf_set_logger(data: number, logger: number): void;
  __burrow_qpdf_set_attempt_recovery(data: number, value: number): void;
  __burrow_qpdf_read_memory(
    data: number,
    description: number,
    buffer: number,
    size: bigint,
    password: number,
  ): number;
  __burrow_qpdf_has_error(data: number): number;
  __burrow_qpdf_more_warnings(data: number): number;
  __burrow_qpdf_get_error(data: number): number;
  __burrow_qpdf_get_error_code(data: number, error: number): number;
  __burrow_qpdf_get_num_pages(data: number): number;
  __burrow_qpdf_get_page_n(data: number, n: number): number;
  __burrow_qpdf_get_object_by_id(data: number, number: number, generation: number): number;
  __burrow_qpdf_get_trailer(data: number): number;
  __burrow_qpdf_add_page(data: number, source: number, page: number, first: number): number;
  __burrow_qpdf_init_write_memory(data: number): number;
  __burrow_qpdf_set_deterministic_id(data: number, value: number): void;
  /** The mode is decided in Rust and passed through unexamined. ADR 0009 §2. */
  __burrow_qpdf_set_object_stream_mode(data: number, mode: number): void;
  __burrow_qpdf_write(data: number): number;
  __burrow_qpdf_get_buffer_length(data: number): number;
  __burrow_qpdf_get_buffer(data: number): number;
  __burrow_qpdf_oh_get_key(data: number, oh: number, key: number): number;
  __burrow_qpdf_oh_get_type_code(data: number, oh: number): number;
  __burrow_qpdf_oh_get_int_value(data: number, oh: number): bigint;
  __burrow_qpdf_oh_new_integer(data: number, value: bigint): number;
  __burrow_qpdf_oh_replace_key(data: number, oh: number, key: number, item: number): void;
  __burrow_qpdf_remove_page(data: number, page: number): number;
  __burrow_qpdf_add_page_at(
    data: number,
    source: number,
    page: number,
    before: number,
    refpage: number,
  ): number;
  /** `(object_number << 32) | generation` — both halves in one call. See `bridge.js`. */
  __burrow_qpdf_oh_object(data: number, oh: number): bigint;
  __burrow_qpdf_oh_unparse_resolved(data: number, oh: number): number;
  __burrow_qpdf_oh_get_name(data: number, oh: number): number;
  __burrow_qpdf_oh_remove_key(data: number, oh: number, key: number): void;
  __burrow_qpdf_oh_get_array_n_items(data: number, oh: number): number;
  __burrow_qpdf_oh_get_array_item(data: number, oh: number, at: number): number;
  __burrow_qpdf_oh_erase_item(data: number, oh: number, at: number): void;
  __burrow_qpdf_oh_set_array_item(data: number, oh: number, at: number, item: number): void;
  __burrow_qpdf_oh_get_dict(data: number, oh: number): number;
  __burrow_qpdf_oh_new_null(data: number): number;
  __burrow_qpdf_oh_replace_stream_data(
    data: number,
    stream: number,
    bytes: Uint8Array,
    filter: number,
    decodeParms: number,
  ): boolean;
  __burrow_qpdf_copy_c_string(ptr: number): Uint8Array;
  /** `null` when qpdf reported an error. */
  __burrow_qpdf_oh_page_content(data: number, page: number): Uint8Array | null;
  /** `null` when qpdf reported an error **or could not decode the stream**. */
  __burrow_qpdf_oh_stream_data(data: number, oh: number): Uint8Array | null;
  __burrow_qpdf_oh_release(data: number, oh: number): void;
  /** The only bridge function that carries bytes OUT of an engine heap. */
  __burrow_qpdf_copy_out(ptr: number, len: number): Uint8Array;
  __burrow_qpdf_global_set_uint32(param: number, value: number): number;
  __burrow_qpdflogger_create(): number;
  __burrow_qpdflogger_discard_all(logger: number, destination: number): void;
  __burrow_qpdf_heap_pages(): number;
}
