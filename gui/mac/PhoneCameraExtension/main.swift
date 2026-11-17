import CoreMediaIO
import Foundation

// The camera extension: publishes the cameras the app asks for and serves the frames the app feeds.
let providerSource = CameraProviderSource(clientQueue: nil)
CMIOExtensionProvider.startService(provider: providerSource.provider)
CFRunLoopRun()
