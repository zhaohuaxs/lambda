// Bridge between Lambda's Rust engine and the vendored 7-Zip 26.04 sources.
//
// Bern loads the application's Rust crates into one binary and gives no way to
// add a second native library, so the 7zz command line front-end
// (CPP/7zip/UI/Console) is linked into that same module. `main` there is a
// plain unmangled C symbol; calling it in-process with an argv is the smallest
// reliable way to reuse 7-Zip's own extract/update pipeline (callbacks, path
// sanitising, codecs, encryption) without reimplementing it.
//
// stdout/stderr are redirected to a scratch file for the duration of the
// call: 7z's global CStdOutStream objects hold `stdout`/`stderr`, and the file
// descriptor swap keeps them usable while giving Rust a readable log. A mutex
// serialises calls because 7z keeps global state.
//
// Windows builds the same sources with MSVC: the argv path is forced through
// Z7_LAMBDA_INPROCESS (see Main.cpp), and `lambda7z_engine_main` is the C
// wrapper defined in MainAr.cpp.

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#include <sys/stat.h>
#include <windows.h>
#else
#include <csignal>
#include <fcntl.h>
#include <signal.h>
#include <unistd.h>
#endif

// Defined by CPP/7zip/UI/Console/MainAr.cpp (Z7_LAMBDA_INPROCESS).
extern "C" int lambda7z_engine_main(int numArgs, char *args[]);

// Set by the watcher thread to abort the running console operation, exactly
// like the Ctrl-C handler does (the callbacks poll it and throw).
namespace NConsoleClose
{
    extern unsigned g_BreakCounter;
}

typedef int (*lambda7z_sink_fn)(void *user, const unsigned char *data, size_t size);

static std::mutex g_lambda7z_mutex;
static std::once_flag g_lambda7z_unbuffered;
#ifndef _WIN32
// 7-Zip's `-so` writes into a pipe; a closed read end must not terminate the
// whole application. The handler is restored when the call returns instead of
// leaking the process-wide change into the host.
struct Lambda7zSigpipeScope
{
    struct sigaction old;
    bool active;
    Lambda7zSigpipeScope()
    {
        active = sigaction(SIGPIPE, nullptr, &old) == 0;
        if (active) {
            struct sigaction ignore {};
            ignore.sa_handler = SIG_IGN;
            sigemptyset(&ignore.sa_mask);
            sigaction(SIGPIPE, &ignore, nullptr);
        }
    }
    ~Lambda7zSigpipeScope()
    {
        if (active) sigaction(SIGPIPE, &old, nullptr);
    }
};
#endif

#ifdef _WIN32
#define lambda7z_dup _dup
#define lambda7z_dup2 _dup2
#define lambda7z_close _close
#define lambda7z_read _read

// Rust hands paths over as UTF-8. The CRT's narrow open would read those
// bytes as the ANSI code page, so a non-ASCII archive name silently created
// the log under a mojibake name and Rust then read an empty file (a Chinese
// 7z showed up as an empty archive). Go through the wide API instead.
static std::wstring lambda7z_wide_path(const char *utf8)
{
    const int length = static_cast<int>(std::strlen(utf8));
    if (length <= 0) return std::wstring();
    const int size = MultiByteToWideChar(CP_UTF8, 0, utf8, length, nullptr, 0);
    if (size <= 0) return std::wstring();
    std::wstring wide(static_cast<size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8, length, wide.data(), size);
    return wide;
}

static int lambda7z_open_log(const char *path)
{
    const std::wstring wide = lambda7z_wide_path(path);
    // O_EXCL: the name is unique per call, so an existing file (or a planted
    // link) must never be truncated. O_NOINHERIT keeps the fd out of children.
    return _wopen(wide.c_str(),
                  _O_WRONLY | _O_CREAT | _O_EXCL | _O_BINARY | _O_NOINHERIT,
                  _S_IREAD | _S_IWRITE);
}

static FILE *lambda7z_fopen_read(const char *path)
{
    const std::wstring wide = lambda7z_wide_path(path);
    return _wfopen(wide.c_str(), L"rb");
}

static int lambda7z_pipe(int fds[2])
{
    return _pipe(fds, 1 << 16, _O_BINARY);
}

static bool lambda7z_std_handle_valid(int fd)
{
    const intptr_t handle = _get_osfhandle(fd);
    return handle != -1 && handle != -2;
}

// Explorer starts a GUI process without a console: the UCRT leaves stdout
// and stderr bound to invalid handles, and fwrite() keeps failing on them
// even after dup2() gives fd 1 a real file (every archive listed empty).
// Rebuild the broken streams on NUL; the callers then redirect the stream's
// own fd to the log or the preview pipe.
static void lambda7z_repair_std_streams()
{
    if (!lambda7z_std_handle_valid(0)) _wfreopen(L"NUL", L"r", stdin);
    if (!lambda7z_std_handle_valid(1)) _wfreopen(L"NUL", L"w", stdout);
    if (!lambda7z_std_handle_valid(2)) _wfreopen(L"NUL", L"w", stderr);
}

static int lambda7z_stdout_fd()
{
    return _fileno(stdout);
}

static int lambda7z_stderr_fd()
{
    return _fileno(stderr);
}
#else
#define lambda7z_dup dup
#define lambda7z_dup2 dup2
#define lambda7z_close close
#define lambda7z_read read

static int lambda7z_open_log(const char *path)
{
    return open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
}

static FILE *lambda7z_fopen_read(const char *path)
{
    return std::fopen(path, "rb");
}

static int lambda7z_pipe(int fds[2])
{
    return pipe(fds);
}

static int lambda7z_stdout_fd()
{
    return STDOUT_FILENO;
}

static int lambda7z_stderr_fd()
{
    return STDERR_FILENO;
}
#endif

static void make_stdio_unbuffered()
{
    std::call_once(g_lambda7z_unbuffered, [] {
        std::setvbuf(stdout, nullptr, _IONBF, 0);
        std::setvbuf(stderr, nullptr, _IONBF, 0);
        #ifdef _WIN32
        // Keep names byte-exact: the log is parsed as UTF-8 by Rust.
        _setmode(lambda7z_stdout_fd(), _O_BINARY);
        _setmode(lambda7z_stderr_fd(), _O_BINARY);
        #endif
    });
}

static void read_log_tail(const char *log_path, char *message, size_t message_cap)
{
    if (!message || message_cap <= 1) return;
    FILE *log = lambda7z_fopen_read(log_path);
    if (!log) return;
#ifdef _WIN32
    if (_fseeki64(log, 0, SEEK_END) == 0) {
        const __int64 size = _ftelli64(log);
        const __int64 wanted = static_cast<__int64>(message_cap - 1);
        const __int64 take = size > wanted ? wanted : size;
        if (take > 0 && _fseeki64(log, size - take, SEEK_SET) == 0) {
#else
    if (fseeko(log, 0, SEEK_END) == 0) {
        const off_t size = ftello(log);
        const off_t wanted = static_cast<off_t>(message_cap - 1);
        const off_t take = size > wanted ? wanted : size;
        if (take > 0 && fseeko(log, size - take, SEEK_SET) == 0) {
#endif
            const size_t read = std::fread(message, 1, static_cast<size_t>(take), log);
            message[read] = 0;
            // Trim a lone leading LF left by the tail cut.
            if (read > 0 && message[0] == '\n') std::memmove(message, message + 1, read);
        }
    }
    std::fclose(log);
}

typedef int (*lambda7z_cancel_fn)(void *user);

static lambda7z_cancel_fn g_cancel_fn = nullptr;
static void *g_cancel_user = nullptr;

// Called by the patched TestBreakSignal() on the engine thread. The callback
// belongs to the call that is running under the mutex; a cancel therefore
// cannot be lost in a startup window or kill an unrelated operation.
extern "C" int lambda7z_cancel_requested(void)
{
    const lambda7z_cancel_fn fn = g_cancel_fn;
    return fn ? fn(g_cancel_user) : 0;
}

// Registers the current call's cancel callback and clears it on every exit
// path (including the log-open failure below).
struct Lambda7zCancelScope
{
    Lambda7zCancelScope(lambda7z_cancel_fn fn, void *user)
    {
        g_cancel_fn = fn;
        g_cancel_user = user;
    }
    ~Lambda7zCancelScope()
    {
        g_cancel_fn = nullptr;
        g_cancel_user = nullptr;
    }
};

static void lambda7z_secure_zero(void *data, size_t size)
{
    #ifdef _WIN32
    SecureZeroMemory(data, size);
    #else
    // bionic in NDK 28 does not declare explicit_bzero for every API level;
    // volatile writes are not optimized away and are portable.
    volatile unsigned char *bytes = static_cast<volatile unsigned char *>(data);
    while (size > 0) {
        *bytes++ = 0;
        --size;
    }
    #endif
}

static void lambda7z_wipe_strings(std::vector<std::string> &items)
{
    for (auto &item : items) {
        if (item.empty()) continue;
        lambda7z_secure_zero(item.data(), item.size());
    }
}

extern "C" int lambda7z_run(
    int argc,
    const char *const *argv,
    const char *log_path,
    lambda7z_cancel_fn cancel,
    void *cancel_user,
    char *message,
    size_t message_cap)
{
    std::lock_guard<std::mutex> guard(g_lambda7z_mutex);
    NConsoleClose::g_BreakCounter = 0;
    Lambda7zCancelScope cancel_scope(cancel, cancel_user);
    if (message && message_cap > 0) message[0] = 0;
    if (!argv || !log_path || argc <= 0) return 7;

    std::vector<std::string> owned;
    owned.reserve(static_cast<size_t>(argc));
    for (int i = 0; i < argc; ++i) owned.emplace_back(argv[i] ? argv[i] : "");
    std::vector<char *> args;
    args.reserve(owned.size() + 1);
    for (auto &item : owned) args.push_back(item.data());
    args.push_back(nullptr);

    #ifdef _WIN32
    lambda7z_repair_std_streams();
    #endif
    const int out_fd = lambda7z_stdout_fd();
    const int err_fd = lambda7z_stderr_fd();
    const int saved_out = lambda7z_dup(out_fd);
    const int saved_err = lambda7z_dup(err_fd);
    const int log_fd = lambda7z_open_log(log_path);
    if (log_fd < 0) {
        // Without a log Rust cannot read the listing back; fail loudly instead
        // of running with the output going to NUL and reporting an empty
        // archive (a read-only archive directory used to do exactly that).
        if (saved_out >= 0) lambda7z_close(saved_out);
        if (saved_err >= 0) lambda7z_close(saved_err);
        lambda7z_wipe_strings(owned);
        return 9;
    }
    // On some Android processes fd 1/2 are closed or /dev/null; never
    // redirect unless both can be restored afterwards.
    const bool redirect = saved_out >= 0 && saved_err >= 0;

    std::fflush(nullptr);
    if (redirect) {
        // 7-Zip's console UI writes progress into stdio but never flushes:
        // CUpdateCallbackConsole::NeedFlush is constructed false and nothing
        // sets it. With fd 1 on a regular file stdio goes fully buffered and
        // the percentage would sit in a 4 KB buffer until main() returns, so
        // Rust's progress watcher reads nothing the whole run. Disable
        // buffering so every refresh reaches the log immediately.
        make_stdio_unbuffered();
        lambda7z_dup2(log_fd, out_fd);
        lambda7z_dup2(log_fd, err_fd);
    }

    const int code = lambda7z_engine_main(argc, args.data());

    std::fflush(nullptr);
    if (redirect) {
        lambda7z_close(log_fd);
        lambda7z_dup2(saved_out, out_fd);
        lambda7z_dup2(saved_err, err_fd);
    } else if (log_fd >= 0) {
        lambda7z_close(log_fd);
    }
    if (saved_out >= 0) lambda7z_close(saved_out);
    if (saved_err >= 0) lambda7z_close(saved_err);

    // Keep the tail of the log: 7z prints the reason for a failure there.
    read_log_tail(log_path, message, message_cap);
    lambda7z_wipe_strings(owned);
    return code;
}

// Same serialized call as lambda7z_run, but fd 1 is a pipe and every stdout
// block goes to the Rust sink. stderr still goes to the log so a failure
// reason survives. Used by archive content preview (`-so`).
extern "C" int lambda7z_run_stream(
    int argc,
    const char *const *argv,
    const char *log_path,
    lambda7z_cancel_fn cancel,
    void *cancel_user,
    lambda7z_sink_fn sink,
    void *user,
    char *message,
    size_t message_cap)
{
    std::lock_guard<std::mutex> guard(g_lambda7z_mutex);
    NConsoleClose::g_BreakCounter = 0;
    Lambda7zCancelScope cancel_scope(cancel, cancel_user);
    if (message && message_cap > 0) message[0] = 0;
    if (!argv || !log_path || argc <= 0 || !sink) return 7;

    std::vector<std::string> owned;
    owned.reserve(static_cast<size_t>(argc));
    for (int i = 0; i < argc; ++i) owned.emplace_back(argv[i] ? argv[i] : "");
    std::vector<char *> args;
    args.reserve(owned.size() + 1);
    for (auto &item : owned) args.push_back(item.data());
    args.push_back(nullptr);

    // A closed pipe write end must not terminate the whole application.
    #ifndef _WIN32
    Lambda7zSigpipeScope sigpipe_scope;
    #else
    lambda7z_repair_std_streams();
    #endif

    const int out_fd = lambda7z_stdout_fd();
    const int err_fd = lambda7z_stderr_fd();
    const int saved_out = lambda7z_dup(out_fd);
    const int saved_err = lambda7z_dup(err_fd);
    const int log_fd = lambda7z_open_log(log_path);
    if (log_fd < 0) {
        if (saved_out >= 0) lambda7z_close(saved_out);
        if (saved_err >= 0) lambda7z_close(saved_err);
        lambda7z_wipe_strings(owned);
        return 9;
    }
    int stream_fds[2] = {-1, -1};
    const bool can_stream =
        saved_out >= 0 && saved_err >= 0 && lambda7z_pipe(stream_fds) == 0;

    if (!can_stream) {
        if (stream_fds[0] >= 0) lambda7z_close(stream_fds[0]);
        if (stream_fds[1] >= 0) lambda7z_close(stream_fds[1]);
        if (log_fd >= 0) lambda7z_close(log_fd);
        if (saved_out >= 0) lambda7z_close(saved_out);
        if (saved_err >= 0) lambda7z_close(saved_err);
        if (message && message_cap > 1) {
            std::snprintf(message, message_cap, "无法建立预览管道");
        }
        lambda7z_wipe_strings(owned);
        return 8;
    }

    make_stdio_unbuffered();
    std::fflush(nullptr);
    lambda7z_dup2(stream_fds[1], out_fd);
    lambda7z_dup2(log_fd, err_fd);
    lambda7z_close(stream_fds[1]);

    std::thread reader([read_fd = stream_fds[0], sink, user] {
        std::vector<unsigned char> buffer(64 * 1024);
        bool keep = true;
        while (keep) {
            const long read_size = static_cast<long>(
                lambda7z_read(read_fd, buffer.data(), static_cast<unsigned>(buffer.size())));
            if (read_size > 0) {
                if (sink(user, buffer.data(), static_cast<size_t>(read_size)) == 0) {
                    keep = false;
                }
            } else if (read_size == 0) {
                break;
            } else if (errno != EINTR) {
                break;
            }
        }
        lambda7z_close(read_fd);
    });

    const int code = lambda7z_engine_main(argc, args.data());

    std::fflush(nullptr);
    lambda7z_close(log_fd);
    // Closing the stdout duplicate (the pipe write end) lets the reader finish.
    lambda7z_dup2(saved_out, out_fd);
    lambda7z_dup2(saved_err, err_fd);
    reader.join();
    lambda7z_close(saved_out);
    lambda7z_close(saved_err);

    // Keep the tail of the log: 7z prints the reason for a failure there.
    read_log_tail(log_path, message, message_cap);
    lambda7z_wipe_strings(owned);
    return code;
}
