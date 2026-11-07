@echo off
rem Compile Lambda's generated WinUI 3 app without /MP.
rem
rem The generated build.bat uses `cl /MP`, which starts one cl.exe per logical
rem processor (12 here). Each process reserves commit for the ~3.1 GB PCH, so
rem on a 16 GB machine with a small page file the build fails with C3859 /
rem system error 1455 / C1076. Compiling the sources one at a time keeps the
rem peak commit low enough.
rem
rem The same pass injects the MSVC link response file that sevenzip-src's
rem build.rs writes for the requested profile, plus /guard:cf. 7-Zip's codec
rem registration objects must reach the final link directly, and MSVC has no
rem `ld -r` to merge them into one archive member.
rem
rem Usage: build-sequential.cmd [release|debug]   (default: release)
rem
rem Run this from a normal PowerShell / CMD / Developer Command Prompt (not an
rem agent sandbox that blocks reg.exe). `bern build` regenerates build.bat and
rem puts /MP back, so this script patches it again on every run.
setlocal EnableExtensions
cd /d "%~dp0"

set "PROFILE=%~1"
if "%PROFILE%"=="" set "PROFILE=release"
if /i not "%PROFILE%"=="release" if /i not "%PROFILE%"=="debug" (
    echo [!] usage: build-sequential.cmd [release^|debug]
    exit /b 1
)

set "BAT=%CD%\target\bern\windows\app\build.bat"
if not exist "%BAT%" (
    echo [!] build.bat not found. Run "bern build --target windows" once first
    echo     to generate the native project ^(it may fail at the C++ step^).
    exit /b 1
)

set "RSP=%CD%\target\bern-glue-windows\%PROFILE%\lambda7z-msvc.rsp"
if not exist "%RSP%" (
    echo [!] %RSP% not found.
    echo     Build the Rust glue for the %PROFILE% profile first.
    exit /b 1
)

powershell -NoProfile -Command "$s=[IO.File]::ReadAllText('%BAT%'); $n=$s.Replace(' /MP ',' '); $q=[char]34; $n=[regex]::Replace($n, (' @' + $q + '[^' + $q + ']*lambda7z-msvc\.rsp' + $q), ''); $link='link /nologo %%LINKOPT%%'; if(-not $n.Contains($link)){ Write-Host '[!] link line not found'; exit 1 }; $n=$n.Replace($link, $link + ' /guard:cf @' + $q + '%RSP%' + $q); [IO.File]::WriteAllText('%BAT%',$n); Write-Host ('[*] profile: %PROFILE%'); Write-Host ('[*] 7-Zip objects: %RSP%')"
if errorlevel 1 exit /b 1

call "%BAT%"
exit /b %ERRORLEVEL%
