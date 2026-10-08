# The page calls these by name; they must survive shrinking.
-keepclassmembers class app.dastbedast.Bridge {
    @android.webkit.JavascriptInterface <methods>;
}
