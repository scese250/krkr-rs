-keep class dev.krkr.rs.KrkrGameActivity {
    <init>();
}

# pendingGameDir is deliberately left out.
-keepclassmembers class dev.krkr.rs.KrkrGameActivity {
    static java.lang.String pendingLogLevel;
}

-keep class dev.krkr.rs.MainActivity {
    <init>();
}

# A class the manifest never starts.
-keep class dev.krkr.rs.StaleActivity {
    <init>();
}
