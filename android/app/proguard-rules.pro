# The names this app resolves at runtime. R8 renames and removes everything it
# cannot see used, so every name that is looked up by name - by the framework
# from the manifest, or by the native library through JNI - needs a rule here.
#
# The rules of the libraries the app depends on are not repeated: the game
# activity artifact keeps `androidx.core.graphics.Insets` and
# `androidx.core.view.WindowInsetsCompat`, the startup library keeps the
# initializer classes it instantiates from the manifest, and the coroutine
# library keeps the service interfaces it loads with ServiceLoader.

# The engine Activity. The framework instantiates the class the manifest names,
# and the native library reads the seven static fields below with
# GetStaticFieldID.
-keep class dev.krkr.rs.KrkrGameActivity {
    <init>();
}

-keepclassmembers class dev.krkr.rs.KrkrGameActivity {
    static java.lang.String pendingGameDir;
    static java.lang.String pendingGameName;
    static java.lang.String pendingStateDir;
    static java.lang.String pendingFrameRate;
    static java.lang.String pendingPreloadArchives;
    static java.lang.String pendingDebugOverlay;
    static java.lang.String pendingLogLevel;
}

# The launcher Activity, instantiated by the framework from the manifest.
-keep class dev.krkr.rs.MainActivity {
    <init>();
}

# The game activity the engine Activity extends. The native glue finds the
# class with FindClass, binds its native methods with RegisterNatives and
# calls the four methods below by name, so the class name, the native method
# names and those four names have to survive.
-keep class com.google.androidgamesdk.GameActivity {
    native <methods>;
    void setWindowFlags(int, int);
    androidx.core.graphics.Insets getWindowInsets(int);
    androidx.core.graphics.Insets getWaterfallInsets();
    void setImeEditorInfoFields(int, int, int);
}

# The text input classes the native glue finds with FindClass; the fields and
# the methods below are read with GetFieldID and GetMethodID.
-keep class com.google.androidgamesdk.gametextinput.State {
    <init>(java.lang.String, int, int, int, int);
    java.lang.String text;
    int selectionStart;
    int selectionEnd;
    int composingRegionStart;
    int composingRegionEnd;
}

-keep class com.google.androidgamesdk.gametextinput.InputConnection {
    void setState(com.google.androidgamesdk.gametextinput.State);
    void setSoftKeyboardActive(boolean, int);
    void restartInput();
}

# The classes the framework instantiates from the merged manifest: the
# application component factory, the provider that shares the logs, and the
# components the startup and the profile installer libraries add.
-keep class androidx.core.app.CoreComponentFactory {
    <init>(...);
}

-keep class androidx.core.content.FileProvider {
    <init>(...);
}

-keep class androidx.startup.InitializationProvider {
    <init>(...);
}

-keep class androidx.profileinstaller.ProfileInstallReceiver {
    <init>(...);
}
