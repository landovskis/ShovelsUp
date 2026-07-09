# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Stack

SwiftUI. Deployment target: iOS 26.5.

Build and run via Xcode, or from the CLI:

```bash
# compile only (no signing required)
xcodebuild build \
  -project ShovelsUp.xcodeproj \
  -scheme ShovelsUp \
  -sdk iphonesimulator \
  CODE_SIGNING_ALLOWED=NO \
  -quiet
```
