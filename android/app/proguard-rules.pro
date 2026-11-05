# The bridge to Rust goes by name through JNA.
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class xyz.hollingworth.tern.core.** { *; }
-dontwarn java.awt.**
