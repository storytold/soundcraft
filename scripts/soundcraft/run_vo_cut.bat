@echo off
setlocal EnableExtensions
cd /d "%~dp0\..\.."

set "PY=python"
if not "%ZVUKACH_PYTHON%"=="" set "PY=%ZVUKACH_PYTHON%"

set "SCRIPT=%~dp0vo_cut_pipeline.py"

REM Default: nachitka, offline keep-last plan, apply to real session.
REM Bind this .bat to a Windows hotkey (PowerToys / AutoHotkey / Shortcut properties).

if "%~1"=="" (
  "%PY%" "%SCRIPT%" --prefer-cached-asr --llm auto --apply
  exit /b %ERRORLEVEL%
)

if /I "%~1"=="plan" (
  "%PY%" "%SCRIPT%" --prefer-cached-asr --llm auto --dry-run
  exit /b %ERRORLEVEL%
)

if /I "%~1"=="apply" (
  "%PY%" "%SCRIPT%" --prefer-cached-asr --llm auto --apply
  exit /b %ERRORLEVEL%
)

if /I "%~1"=="ollama" (
  "%PY%" "%SCRIPT%" --prefer-cached-asr --llm ollama --dry-run
  exit /b %ERRORLEVEL%
)

if /I "%~1"=="ollama-apply" (
  "%PY%" "%SCRIPT%" --prefer-cached-asr --llm ollama --apply
  exit /b %ERRORLEVEL%
)

REM Pass-through: run_vo_cut.bat path\to.wav [--apply] …
"%PY%" "%SCRIPT%" %*
exit /b %ERRORLEVEL%
