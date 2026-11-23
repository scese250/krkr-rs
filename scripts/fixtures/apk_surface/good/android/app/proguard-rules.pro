-keep class dev.krkr.rs.KrkrGameActivity {
    <init>();
}

-keepclassmembers class dev.krkr.rs.KrkrGameActivity {
    static java.lang.String pendingGameDir;
    static java.lang.String pendingLogLevel;
}
