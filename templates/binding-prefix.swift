// This is a harmless import statement. It is used to ensure the FFI xcframework
// module is imported, when multiple uniffi packages are built into one xcframework.
#if canImport({{ ffi_module_name }})
import {{ ffi_module_name }};
#endif
