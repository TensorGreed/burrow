// Minimal C shim over qpdf for the spike.
//
// The point of this file is the catch blocks. qpdf reports every failure by throwing,
// and ADR 0006 flags C++ exceptions as the risk that decides the linking strategy: if a
// throw becomes an abort, we lose qpdf's error reporting and the whole WASM instance
// with it. So every entry point here must convert a throw into a returned code and
// never let one escape.
#include <qpdf/QPDF.hh>
#include <qpdf/QPDFExc.hh>

#include <cstring>
#include <exception>
#include <string>

// Mirrors the shape of burrow-types' Error, deliberately: this is what the real
// engine wrapper would map into a typed Rust error.
enum QpdfProbeStatus {
  QPDF_OK = 0,
  QPDF_MALFORMED = -1,
  QPDF_PASSWORD_REQUIRED = -2,
  QPDF_UNSUPPORTED = -3,
  QPDF_INTERNAL = -4,
};

static thread_local std::string last_message;

extern "C" {

// Returns page count >= 0, or a negative QpdfProbeStatus. Never throws, never aborts.
int qpdf_probe_pages(const unsigned char* data, int len) {
  last_message.clear();
  try {
    QPDF q;
    q.processMemoryFile("spike-input", reinterpret_cast<const char*>(data),
                        static_cast<size_t>(len));
    return static_cast<int>(q.getAllPages().size());
  } catch (QPDFExc const& e) {
    last_message = e.what();
    // qpdf signals an encrypted file it cannot open with a password prompt condition;
    // distinguish it, because the caller must be able to ask for a password.
    if (last_message.find("password") != std::string::npos ||
        last_message.find("invalid password") != std::string::npos) {
      return QPDF_PASSWORD_REQUIRED;
    }
    return QPDF_MALFORMED;
  } catch (std::runtime_error const& e) {
    last_message = e.what();
    return QPDF_MALFORMED;
  } catch (std::exception const& e) {
    last_message = e.what();
    return QPDF_UNSUPPORTED;
  } catch (...) {
    last_message = "non-standard exception";
    return QPDF_INTERNAL;
  }
}

// Diagnostic only. NOTE: in the real engine wrapper this must NOT be forwarded to the
// host -- qpdf messages embed offsets and object numbers derived from the input, and
// burrow-ffi::guard exists precisely to stop input-derived text escaping. Here it is
// used only to prove the exception was genuinely caught and its payload readable.
const char* qpdf_probe_last_message() { return last_message.c_str(); }

int qpdf_probe_version_ok() { return 1; }
}
