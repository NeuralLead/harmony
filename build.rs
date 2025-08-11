
fn main() {
    csbindgen::Builder::default()
        .input_extern_file("src/cs_module.rs")
        .csharp_dll_name("openai_harmony")
        .csharp_class_name("HarmonyBindings")
        .csharp_namespace("OpenAI.Harmony")
        .generate_csharp_file("target/HarmonyBindings.cs")
        .unwrap();
}