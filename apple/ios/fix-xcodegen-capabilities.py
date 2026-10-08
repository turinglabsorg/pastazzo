from pathlib import Path

# XcodeGen 2.45/2.46 serializes nested capability dictionaries as strings.
project = Path(__file__).with_name("Pastazzo.xcodeproj") / "project.pbxproj"
broken = r'SystemCapabilities = "[\"com.apple.ApplicationGroups.iOS\": [\"enabled\": 1]]";'
correct = "SystemCapabilities = { com.apple.ApplicationGroups.iOS = { enabled = 1; }; };"
project.write_text(project.read_text().replace(broken, correct))
