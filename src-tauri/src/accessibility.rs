//! Разрешение Accessibility (macOS): проверка и одноразовый системный промпт.
//!
//! Реализовано лёгким FFI (AX* входят в ApplicationServices.framework, линкуется
//! в build.rs) + `core-foundation` для CFDictionary. На Windows — no-op stubs.

/// Можно ли эмулировать клавиатуру (на не-macOS всегда true).
pub fn is_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        ffi::is_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// При старте: если права нет — показать системный промпт «разрешить управление
/// этим компьютером» (диалог macOS помнит ответ, повторных не будет до
/// изменения пользователем). Возвращает состояние trusted.
pub fn ensure_prompt() -> bool {
    #[cfg(target_os = "macos")]
    {
        if ffi::is_trusted() {
            return true;
        }
        ffi::prompt();
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFMutableDictionary;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: CFStringRef;
    }

    pub fn is_trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// Вызов с kAXTrustedCheckOptionPrompt=true: если прав нет — macOS покажет
    /// диалог и добавит приложение в список Accessibility.
    pub fn prompt() -> bool {
        let mut dict: CFMutableDictionary<CFString, CFBoolean> = CFMutableDictionary::new();
        let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
        dict.set(key, CFBoolean::true_value());
        unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef().cast()) }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn ax_is_process_trusted_callable() {
        // Вызов не должен паниковать; значение зависит от настроек хоста.
        let trusted: bool = super::is_trusted();
        println!("AXIsProcessTrusted() = {trusted}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stub_is_trusted_on_other_platforms() {
        assert!(super::is_trusted());
    }
}
