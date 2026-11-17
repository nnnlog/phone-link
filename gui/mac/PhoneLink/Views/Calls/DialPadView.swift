import PhoneLinkKit
import SwiftUI

/// The Calls toolbar's keypad: a number typed or tapped in, and *Call*, which places the call on the
/// phone.
struct DialPadView: View {
    @Environment(CallControlModel.self) private var model
    @State private var number = ""

    private static let keys: [(String, String)] = [
        ("1", ""), ("2", "ABC"), ("3", "DEF"),
        ("4", "GHI"), ("5", "JKL"), ("6", "MNO"),
        ("7", "PQRS"), ("8", "TUV"), ("9", "WXYZ"),
        ("*", ""), ("0", "+"), ("#", ""),
    ]

    var body: some View {
        VStack(spacing: 14) {
            HStack {
                TextField("Phone number", text: $number)
                    .textFieldStyle(.roundedBorder)
                    .font(.title3.monospacedDigit())
                    .onSubmit(call)
                Button {
                    if !number.isEmpty { number.removeLast() }
                } label: {
                    Image(systemName: "delete.left")
                }
                .buttonStyle(.borderless)
                .disabled(number.isEmpty)
                .help("Delete")
            }

            LazyVGrid(columns: Array(repeating: GridItem(.fixed(64), spacing: 12), count: 3), spacing: 10) {
                ForEach(Self.keys, id: \.0) { key, letters in
                    Button {
                        number.append(key)
                    } label: {
                        VStack(spacing: 0) {
                            Text(key)
                                .font(.title2)
                            // Every key keeps the letters' line, so the digits line up.
                            Text(letters.isEmpty ? " " : letters)
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                        .frame(width: 56, height: 44)
                    }
                    .buttonStyle(.bordered)
                }
            }

            Button(action: call) {
                Label("Call", systemImage: "phone.fill")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .tint(.green)
            .controlSize(.large)
            .disabled(number.isEmpty || !model.canDial)

            Text("Calls go through your phone.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(16)
        .frame(width: 260)
    }

    private func call() {
        guard !number.isEmpty, model.canDial else { return }
        let dialed = number
        Task {
            await model.dial(dialed)
            if model.dialNotice == nil && model.alert == nil {
                number = ""
            }
        }
    }
}
