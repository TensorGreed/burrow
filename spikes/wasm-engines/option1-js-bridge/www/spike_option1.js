let wasm_bindgen = (function(exports) {
    let script_src;
    if (typeof document !== 'undefined' && document.currentScript !== null) {
        script_src = new URL(document.currentScript.src, location.href).toString();
    }

    /**
     * What an engine call returns to Rust: a page count, or a typed error.
     */
    class Outcome {
        static __wrap(ptr) {
            const obj = Object.create(Outcome.prototype);
            obj.__wbg_ptr = ptr;
            OutcomeFinalization.register(obj, obj.__wbg_ptr, obj);
            return obj;
        }
        __destroy_into_raw() {
            const ptr = this.__wbg_ptr;
            this.__wbg_ptr = 0;
            OutcomeFinalization.unregister(this);
            return ptr;
        }
        free() {
            const ptr = this.__destroy_into_raw();
            wasm.__wbg_outcome_free(ptr, 0);
        }
        /**
         * @returns {SpikeError}
         */
        get error() {
            const ret = wasm.outcome_error(this.__wbg_ptr);
            return ret;
        }
        /**
         * @returns {boolean}
         */
        get is_ok() {
            const ret = wasm.outcome_is_ok(this.__wbg_ptr);
            return ret !== 0;
        }
        /**
         * @returns {number}
         */
        get pages() {
            const ret = wasm.outcome_pages(this.__wbg_ptr);
            return ret;
        }
    }
    if (Symbol.dispose) Outcome.prototype[Symbol.dispose] = Outcome.prototype.free;
    exports.Outcome = Outcome;

    /**
     * Mirrors the shape of `burrow_types::Error`, so the spike exercises the real mapping.
     * @enum {0 | 1 | 2 | 3 | 4}
     */
    const SpikeError = Object.freeze({
        Ok: 0, "0": "Ok",
        Malformed: 1, "1": "Malformed",
        PasswordRequired: 2, "2": "PasswordRequired",
        Unsupported: 3, "3": "Unsupported",
        Internal: 4, "4": "Internal",
    });
    exports.SpikeError = SpikeError;

    /**
     * Deliberately panic, to prove what a Rust panic does to the worker.
     *
     * On `wasm32-unknown-unknown` a panic **aborts**; there is no unwinding, so
     * `catch_unwind` cannot help. The page must detect the dead worker and respawn, which
     * is what `apps/web/CLAUDE.md` requires and what the Playwright test asserts.
     */
    function deliberate_panic() {
        wasm.deliberate_panic();
    }
    exports.deliberate_panic = deliberate_panic;

    /**
     * PDFium reports failure as a negative sentinel from the bridge; map it to a type.
     * @param {Uint8Array} bytes
     * @returns {Outcome}
     */
    function open_with_pdfium(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.open_with_pdfium(ptr0, len0);
        return Outcome.__wrap(ret);
    }
    exports.open_with_pdfium = open_with_pdfium;

    /**
     * qpdf returns the shim's status codes directly.
     * @param {Uint8Array} bytes
     * @returns {Outcome}
     */
    function open_with_qpdf(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.open_with_qpdf(ptr0, len0);
        return Outcome.__wrap(ret);
    }
    exports.open_with_qpdf = open_with_qpdf;

    /**
     * @returns {string}
     */
    function version() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.version();
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    exports.version = version;
    function __wbg_get_imports() {
        const import0 = {
            __proto__: null,
            __wbg___wbindgen_throw_5d9e815e6fdf150f: function(arg0, arg1) {
                throw new Error(getStringFromWasm0(arg0, arg1));
            },
            __wbg_pdfiumPageCount_5f5ce72670a26999: function() { return handleError(function (arg0, arg1) {
                const ret = pdfiumPageCount(getArrayU8FromWasm0(arg0, arg1));
                return ret;
            }, arguments); },
            __wbg_qpdfPageCount_a954f65fce5fba85: function() { return handleError(function (arg0, arg1) {
                const ret = qpdfPageCount(getArrayU8FromWasm0(arg0, arg1));
                return ret;
            }, arguments); },
            __wbindgen_init_externref_table: function() {
                const table = wasm.__wbindgen_externrefs;
                const offset = table.grow(4);
                table.set(0, undefined);
                table.set(offset + 0, undefined);
                table.set(offset + 1, null);
                table.set(offset + 2, true);
                table.set(offset + 3, false);
            },
        };
        return {
            __proto__: null,
            "./spike_option1_bg.js": import0,
        };
    }

    const OutcomeFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_outcome_free(ptr, 1));

    function addToExternrefTable0(obj) {
        const idx = wasm.__externref_table_alloc();
        wasm.__wbindgen_externrefs.set(idx, obj);
        return idx;
    }

    function getArrayU8FromWasm0(ptr, len) {
        ptr = ptr >>> 0;
        return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
    }

    function getStringFromWasm0(ptr, len) {
        return decodeText(ptr >>> 0, len);
    }

    let cachedUint8ArrayMemory0 = null;
    function getUint8ArrayMemory0() {
        if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
            cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
        }
        return cachedUint8ArrayMemory0;
    }

    function handleError(f, args) {
        try {
            return f.apply(this, args);
        } catch (e) {
            const idx = addToExternrefTable0(e);
            wasm.__wbindgen_exn_store(idx);
        }
    }

    function passArray8ToWasm0(arg, malloc) {
        const ptr = malloc(arg.length * 1, 1) >>> 0;
        getUint8ArrayMemory0().set(arg, ptr / 1);
        WASM_VECTOR_LEN = arg.length;
        return ptr;
    }

    let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
    cachedTextDecoder.decode();
    function decodeText(ptr, len) {
        return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
    }

    let WASM_VECTOR_LEN = 0;

    let wasmModule, wasmInstance, wasm;
    function __wbg_finalize_init(instance, module) {
        wasmInstance = instance;
        wasm = instance.exports;
        wasmModule = module;
        cachedUint8ArrayMemory0 = null;
        wasm.__wbindgen_start();
        return wasm;
    }

    async function __wbg_load(module, imports) {
        if (typeof Response === 'function' && module instanceof Response) {
            if (!module.ok) {
                throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
            }

            if (typeof WebAssembly.instantiateStreaming === 'function') {
                try {
                    return await WebAssembly.instantiateStreaming(module, imports);
                } catch (e) {
                    const validResponse = expectedResponseType(module.type);

                    if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                        console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                    } else { throw e; }
                }
            }

            const bytes = await module.arrayBuffer();
            return await WebAssembly.instantiate(bytes, imports);
        } else {
            const instance = await WebAssembly.instantiate(module, imports);

            if (instance instanceof WebAssembly.Instance) {
                return { instance, module };
            } else {
                return instance;
            }
        }

        function expectedResponseType(type) {
            switch (type) {
                case 'basic': case 'cors': case 'default': return true;
            }
            return false;
        }
    }

    function initSync(module) {
        if (wasm !== undefined) return wasm;


        if (module !== undefined) {
            if (Object.getPrototypeOf(module) === Object.prototype) {
                ({module} = module)
            } else {
                console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
            }
        }

        const imports = __wbg_get_imports();
        if (!(module instanceof WebAssembly.Module)) {
            module = new WebAssembly.Module(module);
        }
        const instance = new WebAssembly.Instance(module, imports);
        return __wbg_finalize_init(instance, module);
    }

    async function __wbg_init(module_or_path) {
        if (wasm !== undefined) return wasm;


        if (module_or_path !== undefined) {
            if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
                ({module_or_path} = module_or_path)
            } else {
                console.warn('using deprecated parameters for the initialization function; pass a single object instead')
            }
        }

        if (module_or_path === undefined && script_src !== undefined) {
            module_or_path = script_src.replace(/\.js$/, "_bg.wasm");
        }
        const imports = __wbg_get_imports();

        if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
            module_or_path = fetch(module_or_path);
        }

        const { instance, module } = await __wbg_load(await module_or_path, imports);

        return __wbg_finalize_init(instance, module);
    }

    return Object.assign(__wbg_init, { initSync }, exports);
})({ __proto__: null });
