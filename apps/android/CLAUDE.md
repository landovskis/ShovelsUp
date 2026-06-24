# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Stack

Kotlin + Jetpack Compose. Targets **API 37 (Android 17)**, min SDK 26.

Key versions: AGP 9.2.1, Gradle 9.6.0, Kotlin 2.4.0, Compose BOM 2026.06.00.

The `kotlin-android` plugin is absent — AGP 9.0+ has built-in Kotlin support. `kotlinOptions` is also gone; `compileOptions` covers both Java and Kotlin JVM targets.

UI structure: `MainActivity` → `ShovelsUpTheme` → `HomeScreen`. Screens in `app/src/main/kotlin/com/shovelsup/ui/screens/`, theme in `ui/theme/`.

## Commands

```bash
./gradlew assembleDebug     # build debug APK
./gradlew installDebug      # build + install to connected device
./gradlew test              # unit tests
./gradlew connectedCheck    # instrumented tests
```
