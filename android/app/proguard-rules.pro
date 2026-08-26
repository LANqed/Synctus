# The JNI entry points are looked up by name from native code, so R8 must not
# rename or remove them.
-keep class dev.synctus.app.NativeBridge {
    native <methods>;
    *;
}

# Serialisation uses reflection on the generated serializers.
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.**
-keepclassmembers class kotlinx.serialization.json.** {
    *** Companion;
}
-keepclasseswithmembers class kotlinx.serialization.json.** {
    kotlinx.serialization.KSerializer serializer(...);
}
-keep,includedescriptorclasses class dev.synctus.app.**$$serializer { *; }
-keepclassmembers class dev.synctus.app.** {
    *** Companion;
}
-keepclasseswithmembers class dev.synctus.app.** {
    kotlinx.serialization.KSerializer serializer(...);
}
