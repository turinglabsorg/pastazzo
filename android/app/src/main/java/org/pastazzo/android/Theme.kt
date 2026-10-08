package org.pastazzo.android

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

val Citrus = Color(0xFFF2661B)
val CitrusInk = Color(0xFF27170E)

@Composable
fun PastazzoTheme(content: @Composable () -> Unit) {
    val colors = if (isSystemInDarkTheme()) darkColorScheme(
        primary = Color(0xFFFFAD73), onPrimary = CitrusInk,
        background = Color(0xFF171615), onBackground = Color(0xFFF4EEE8),
        surface = Color(0xFF171615), onSurface = Color(0xFFF4EEE8),
        surfaceContainer = Color(0xFF272521),
        secondaryContainer = Color(0xFF3F352B), onSecondaryContainer = Color(0xFFFFE4C6),
    ) else lightColorScheme(
        primary = Color(0xFFB4440D), onPrimary = Color.White,
        background = Color(0xFFFFFBF7), onBackground = Color(0xFF26231F),
        surface = Color(0xFFFFFBF7), onSurface = Color(0xFF26231F),
        surfaceContainer = Color(0xFFF4EEE8),
        secondaryContainer = Color(0xFFFCEBD8), onSecondaryContainer = Color(0xFF3B2E20),
    )
    MaterialTheme(colorScheme = colors, content = content)
}
