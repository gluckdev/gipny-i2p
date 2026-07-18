import java.io.File
import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction

/**
 * Cross-compiles the bundled go-i2p SAM router (`i2p-router/`) into a
 * cgo `c-shared` library exposing JNI entry points (see
 * `i2p-router/android_export.go` + `jni_shim_android.c`), and drops it into
 * `app/src/main/jniLibs/<abi>/libgipnyi2p.so` so the Android Gradle plugin
 * packages it like any other native library.
 *
 * This mirrors [RustPlugin]/[BuildTask] (which build the Rust/Tauri cdylib)
 * but targets the separate `i2p-router` Go module instead. Requires the
 * Android NDK (resolved from `ANDROID_NDK_HOME`/`ANDROID_NDK_ROOT` or
 * `local.properties`) and a Go toolchain with CGO support on `PATH`.
 */
open class GoRouterTask : DefaultTask() {
    @Input
    var rootDirRel: String? = null

    /** Go GOARCH, e.g. "arm64", "amd64". */
    @Input
    var goArch: String? = null

    /** Android ABI dir name under jniLibs, e.g. "arm64-v8a", "x86_64". */
    @Input
    var abi: String? = null

    /** NDK clang triple prefix, e.g. "aarch64-linux-android", "x86_64-linux-android". */
    @Input
    var ndkTriple: String? = null

    /** Minimum supported API level for the NDK clang wrapper name. */
    @Input
    var minSdk: Int = 24

    @TaskAction
    fun build() {
        val rootDirRel = rootDirRel ?: throw GradleException("rootDirRel cannot be null")
        val goArch = goArch ?: throw GradleException("goArch cannot be null")
        val abi = abi ?: throw GradleException("abi cannot be null")
        val ndkTriple = ndkTriple ?: throw GradleException("ndkTriple cannot be null")

        val ndkDir = resolveNdkDir()
            ?: throw GradleException(
                "Android NDK not found; set ANDROID_NDK_HOME/ANDROID_NDK_ROOT or " +
                    "ndk.dir in local.properties"
            )

        val hostTag = when {
            Os.isLinux() -> "linux-x86_64"
            Os.isMac() -> "darwin-x86_64"
            Os.isWindows() -> "windows-x86_64"
            else -> throw GradleException("Unsupported host OS for NDK toolchain")
        }
        val clangSuffix = if (Os.isWindows()) ".cmd" else ""
        val cc = File(
            ndkDir,
            "toolchains/llvm/prebuilt/$hostTag/bin/$ndkTriple$minSdk-clang$clangSuffix"
        )
        if (!cc.exists()) {
            throw GradleException("NDK clang not found at $cc")
        }

        val routerDir = File(project.projectDir, "$rootDirRel/i2p-router")
        val outDir = File(project.projectDir, "src/main/jniLibs/$abi")
        outDir.mkdirs()
        val outFile = File(outDir, "libgipnyi2p.so")

        project.exec {
            workingDir(routerDir)
            executable("go")
            args(
                "build",
                "-buildmode=c-shared",
                "-trimpath",
                "-ldflags", "-s -w",
                "-o", outFile.absolutePath,
                "."
            )
            environment("CGO_ENABLED", "1")
            environment("GOOS", "android")
            environment("GOARCH", goArch)
            environment("CC", cc.absolutePath)
        }.assertNormalExitValue()

        // go build -buildmode=c-shared also emits a .h header we don't need.
        File(outDir, "libgipnyi2p.h").delete()
    }

    /** Resolve the NDK root: env vars first, then `ndk.dir`/`sdk.dir/ndk` in local.properties. */
    private fun resolveNdkDir(): File? {
        System.getenv("ANDROID_NDK_HOME")?.let { return File(it) }
        System.getenv("ANDROID_NDK_ROOT")?.let { return File(it) }
        val localProps = File(project.rootDir, "local.properties")
        if (localProps.exists()) {
            val props = java.util.Properties().apply { localProps.inputStream().use { load(it) } }
            props.getProperty("ndk.dir")?.let { return File(it) }
            val sdkDir = props.getProperty("sdk.dir")
            if (sdkDir != null) {
                val ndkRoot = File(sdkDir, "ndk")
                val versions = ndkRoot.listFiles { f -> f.isDirectory }
                if (!versions.isNullOrEmpty()) return versions.maxByOrNull { it.name }
            }
        }
        return null
    }
}

private object Os {
    fun isLinux() = System.getProperty("os.name").lowercase().contains("linux")
    fun isMac() = System.getProperty("os.name").lowercase().contains("mac")
    fun isWindows() = System.getProperty("os.name").lowercase().contains("windows")
}
