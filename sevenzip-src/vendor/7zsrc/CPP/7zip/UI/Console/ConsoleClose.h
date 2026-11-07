// ConsoleClose.h

#ifndef ZIP7_INC_CONSOLE_CLOSE_H
#define ZIP7_INC_CONSOLE_CLOSE_H

#ifdef Z7_LAMBDA_INPROCESS
// Defined by bridge/lambda7z.cpp. Checks the cancel flag of the current
// in-process call; see the patched TestBreakSignal below.
extern "C" int lambda7z_cancel_requested(void);
#endif

namespace NConsoleClose {

// class CCtrlBreakException {};

#ifdef UNDER_CE

inline bool TestBreakSignal() { return false; }
struct CCtrlHandlerSetter {};

#else

extern unsigned g_BreakCounter;

inline bool TestBreakSignal()
{
#ifdef Z7_LAMBDA_INPROCESS
  // The bridge registers one cancel callback per engine call, so a cancel
  // belongs to exactly the operation that is running and cannot leak into
  // the next one.
  return (g_BreakCounter != 0 || lambda7z_cancel_requested() != 0);
#else
  return (g_BreakCounter != 0);
#endif
}

class CCtrlHandlerSetter Z7_final
{
  #ifndef _WIN32
  void (*memo_sig_int)(int);
  void (*memo_sig_term)(int);
  #endif
public:
  CCtrlHandlerSetter();
  ~CCtrlHandlerSetter();
};

#endif

}

#endif
