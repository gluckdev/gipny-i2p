import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction
import java.io.File

/**
 * Stages the prebuilt i2pd JNI library for one ABI into src/main/jniLibs.
 *
 * i2pd is C++ with boost and OpenSSL; building it from Gradle on every APK build
 * would add hours per ABI, so it is built once in CI
 * (release.yml and i2p-embed.yml, from android-router/jni) and consumed here.
 * The predecessor task cross-compiled the Go router inline, which was possible
 * only because pure Go builds in seconds.
 *
 * Source directory, first match wins:
 *   1. $GIPNY_I2PD_JNILIBS/<abi>/libi2pd.so   — what CI sets
 *   2. android-router/prebuilt/<abi>/libi2pd.so — a local drop for dev builds
 *
 * Missing input is a hard failure with the fix spelled out: a silently absent
 * router produces an APK that installs, starts, and never connects, which is the
 * failure mode this whole migration exists to end.
 */
abstract class I2pdRouterTask : DefaultTask() {
    @get:Input
    var rootDirRel: String = "../../../.."

    @get:Input
    var abi: String = ""

    @TaskAction
    fun stage() {
        val root = File(project.projectDir, rootDirRel).canonicalFile
        val fromEnv = System.getenv("GIPNY_I2PD_JNILIBS")?.takeIf { it.isNotBlank() }
        val candidates = buildList {
            if (fromEnv != null) add(File(File(fromEnv), "$abi/libi2pd.so"))
            add(File(root, "android-router/prebuilt/$abi/libi2pd.so"))
        }
        val source = candidates.firstOrNull { it.isFile }
            ?: throw GradleException(
                "no prebuilt libi2pd.so for $abi. Looked in:\n" +
                    candidates.joinToString("\n") { "  - $it" } +
                    "\nTake it from release.yml's router-android artifact and drop it in " +
                    "android-router/prebuilt/$abi/, or set GIPNY_I2PD_JNILIBS to a " +
                    "directory laid out as <abi>/libi2pd.so. " +
                    "Pass -PskipRouter to build an APK without a router (it will not connect)."
            )

        val destDir = File(project.projectDir, "src/main/jniLibs/$abi")
        destDir.mkdirs()
        val dest = File(destDir, "libi2pd.so")
        if (dest.exists() && dest.length() == source.length() &&
            dest.lastModified() >= source.lastModified()
        ) {
            logger.lifecycle("i2pd router for $abi already staged")
            return
        }
        source.copyTo(dest, overwrite = true)
        logger.lifecycle("staged i2pd router for $abi (${source.length() / 1024} KiB) from $source")
    }
}
