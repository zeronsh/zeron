# R8 rules for the release build (debug is not minified).
#
# The Rust core is reached through UniFFI's generated Kotlin (uniffi.**) on
# top of JNA. Both work by reflection and from native code: JNA reads
# Structure fields by name (and their @FieldOrder), instantiates Structure
# and Callback classes, finds `callback` methods on Callback interfaces, and
# its libjnidispatch calls back into com.sun.jna by name. Nothing of either
# may be renamed or removed.

# JNA
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }

# UniFFI bindings: every class, field, method and constructor, including the
# callback-interface vtables (com.sun.jna.Callback), Structure records
# (RustBuffer, UniffiRustCallStatus, ForeignFuture*, VTables), the
# `external`/Native.register methods and the handle maps.
-keep class uniffi.** { *; }
-keep interface uniffi.** { *; }

# Structure.FieldOrder and other annotations JNA reads at run time; generic
# signatures and inner-class info for Structure.ByValue / ByReference.
-keepattributes RuntimeVisibleAnnotations,AnnotationDefault,Signature,InnerClasses,EnclosingMethod

# Anything declaring JNI methods keeps them under their names.
-keepclasseswithmembernames,includedescriptorclasses class * {
    native <methods>;
}
