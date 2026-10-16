@echo off
setlocal EnableExtensions
cd /d "%~dp0\..\.."

set "PY=python"
if not "%ZVUKACH_PYTHON%"=="" set "PY=%ZVUKACH_PYTHON%"

set "SCRIPT=%~dp0vo_dedupe_last.py"
set "MODEL=base"
set "DEVICE=auto"

if "%~1"=="--self-test" (
  "%PY%" "%SCRIPT%" --self-test
  exit /b %ERRORLEVEL%
)

if not exist "input\*.wav" (
  echo No WAV files in input\
  echo Usage: run_dedupe.bat           ^| process input\*.wav
  echo        run_dedupe.bat --self-test
  echo        run_dedupe.bat path\to\file.wav
  exit /b 1
)

if not "%~1"=="" (
  echo Processing %~1
  "%PY%" "%SCRIPT%" "%~1" --model %MODEL% --device %DEVICE% --render
  exit /b %ERRORLEVEL%
)

for %%F in (input\*.wav) do (
  echo === %%F ===
  "%PY%" "%SCRIPT%" "%%F" --model %MODEL% --device %DEVICE% --render
  if errorlevel 1 exit /b %ERRORLEVEL%
)

echo Done.
exit /b 0
