import Carbon
import PhoneLinkEngine

/// The kind of the Mac's current keyboard input source: a keyboard layout or an input method.
///
/// Read on the main thread only: with an input method selected and a window focused, the Text Input
/// Sources calls stop the app (a main-queue assertion) when made from any other thread.
public enum KeyboardInputSource {
    @MainActor
    public static func current() -> MirrorInputSource {
        guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
              let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceType)
        else { return .layout }
        let type = Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue()
        let inputMethod = CFEqual(type, kTISTypeKeyboardInputMode) || CFEqual(type, kTISTypeKeyboardInputMethodModeEnabled)
        return inputMethod ? .inputMethod : .layout
    }
}
