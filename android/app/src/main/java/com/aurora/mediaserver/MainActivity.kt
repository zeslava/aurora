package com.aurora.mediaserver

import android.annotation.SuppressLint
import android.app.Activity
import android.app.UiModeManager
import android.content.pm.ActivityInfo
import android.content.pm.ApplicationInfo
import android.content.res.Configuration
import android.os.Bundle
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.WebChromeClient
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout

// Оболочка Android TV-приложения: WebView с той же страницей, что и .ipk для webOS
// (frontend/webos/index.html). Она хранит адрес сервера и уходит на SPA, которую
// отдаёт сам сервер. На телефоне и планшете — обычная мобильная SPA, как в браузере.
// См. docs/decisions/0009-android-tv-shell.md
class MainActivity : Activity() {

    private val isTv by lazy {
        getSystemService(UiModeManager::class.java).currentModeType ==
            Configuration.UI_MODE_TYPE_TELEVISION
    }

    private lateinit var root: FrameLayout
    private lateinit var webView: WebView

    // Полноэкранное видео (requestFullscreen) WebView отдаёт отдельным View
    private var customView: View? = null
    private var customViewCallback: WebChromeClient.CustomViewCallback? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // На телефоне экран не гаснет только в полноэкранном видео (onShowCustomView)
        if (isTv) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        WebView.setWebContentsDebuggingEnabled(
            applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
        )

        webView = WebView(this)
        webView.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            mediaPlaybackRequiresUserGesture = false
            // Оболочка лежит в assets и открывается по file://
            allowFileAccess = true
            mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
            // Без wide viewport WebView игнорирует meta viewport width=1920 и отдаёт
            // CSS-ширину экрана/density (960px на 1080p-ТВ) — UI вдвое крупнее, чем на webOS
            useWideViewPort = true
            loadWithOverviewMode = true
            // По этому маркеру фронтенд включает ТВ-режим и viewport 1920 (lib/tvMode.ts)
            if (isTv) userAgentString = "$userAgentString AuroraTV"
        }
        webView.webViewClient = WebViewClient()
        webView.webChromeClient = object : WebChromeClient() {
            override fun onShowCustomView(view: View, callback: CustomViewCallback) {
                if (customView != null) {
                    callback.onCustomViewHidden()
                    return
                }
                customView = view
                customViewCallback = callback
                root.addView(view, matchParent())
                webView.visibility = View.GONE
                if (!isTv) {
                    window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
                    hideSystemBars()
                }
            }

            override fun onHideCustomView() {
                val view = customView ?: return
                root.removeView(view)
                customView = null
                customViewCallback = null
                webView.visibility = View.VISIBLE
                webView.requestFocus()
                if (!isTv) {
                    window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
                    showSystemBars()
                }
            }
        }

        root = FrameLayout(this)
        root.addView(webView, matchParent())
        setContentView(root)
        if (isTv) hideSystemBars()

        if (savedInstanceState == null) {
            webView.loadUrl("file:///android_asset/index.html")
        } else {
            webView.restoreState(savedInstanceState)
        }
        webView.requestFocus()
    }

    // Back с пульта не доходит до JS: выходим из полноэкранного видео, иначе идём
    // назад по истории WebView (= истории SPA), и только с первой страницы — из приложения
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (keyCode == KeyEvent.KEYCODE_BACK) {
            when {
                customView != null -> customViewCallback?.onCustomViewHidden()
                webView.canGoBack() -> webView.goBack()
                else -> finish()
            }
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && (isTv || customView != null)) hideSystemBars()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        webView.saveState(outState)
    }

    override fun onResume() {
        super.onResume()
        webView.onResume()
    }

    override fun onPause() {
        webView.onPause()
        super.onPause()
    }

    override fun onDestroy() {
        webView.destroy()
        super.onDestroy()
    }

    // WindowInsetsController только с API 30, а minSdk 24 — оставляем флаги
    @Suppress("DEPRECATION")
    private fun hideSystemBars() {
        window.decorView.systemUiVisibility = (View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
            or View.SYSTEM_UI_FLAG_FULLSCREEN
            or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
            or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
            or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
            or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION)
    }

    @Suppress("DEPRECATION")
    private fun showSystemBars() {
        window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_VISIBLE
    }

    private fun matchParent() = FrameLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.MATCH_PARENT
    )
}
