import SwiftUI

struct ContentView: View {
    var body: some View {
        NavigationStack {
            VStack(spacing: 8) {
                Text("ShovelsUp")
                    .font(.largeTitle)
                    .fontWeight(.bold)
                    .foregroundStyle(.brandPrimary)
                Text("tagline")
                    .font(.body)
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(UIColor.systemBackground))
        }
    }
}

#Preview {
    ContentView()
}
