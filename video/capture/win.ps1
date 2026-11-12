# Window helpers for the isolated captures: they only touch the windows of a
# given PID. PrintWindow renders that window alone (never the desktop), and
# clicks/keys are posted to its message queue, so the real mouse never moves.
if (-not ('JotWin' -as [type])) {
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
public static class JotWin {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int size);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
    public static List<IntPtr> Windows(uint pid) {
        var list = new List<IntPtr>();
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) list.Add(h); return true; }, IntPtr.Zero);
        return list;
    }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
    public static IntPtr TrayHost(uint pid) {
        IntPtr h = IntPtr.Zero;
        while ((h = FindWindowEx(IntPtr.Zero, h, "JotTrayHost", null)) != IntPtr.Zero) { uint p; GetWindowThreadProcessId(h, out p); if (p == pid) return h; }
        return IntPtr.Zero;
    }
    public static int[] Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return new[] { r.L, r.T, r.R - r.L, r.B - r.T }; }
    public static int[] Client(IntPtr h) { RECT r; GetClientRect(h, out r); return new[] { r.R, r.B }; }
    public static void Capture(IntPtr h, string path) {
        RECT w; GetWindowRect(h, out w);
        RECT f; if (DwmGetWindowAttribute(h, 9, out f, Marshal.SizeOf(typeof(RECT))) != 0) f = w;
        int W = w.R - w.L, H = w.B - w.T;
        using (var bmp = new Bitmap(W, H, PixelFormat.Format32bppArgb)) {
            using (var g = Graphics.FromImage(bmp)) { IntPtr dc = g.GetHdc(); PrintWindow(h, dc, 2); g.ReleaseHdc(dc); }
            var crop = new Rectangle(f.L - w.L, f.T - w.T, f.R - f.L, f.B - f.T);
            using (var c = bmp.Clone(crop, PixelFormat.Format32bppArgb)) c.Save(path, ImageFormat.Png);
        }
    }
    static IntPtr XY(int x, int y) { return (IntPtr)((y << 16) | (x & 0xFFFF)); }
    public static void Click(IntPtr h, int x, int y) {
        PostMessage(h, 0x0200, IntPtr.Zero, XY(x, y));
        System.Threading.Thread.Sleep(60);
        PostMessage(h, 0x0201, (IntPtr)1, XY(x, y));
        System.Threading.Thread.Sleep(60);
        PostMessage(h, 0x0202, IntPtr.Zero, XY(x, y));
    }
    public static void Drag(IntPtr h, int x0, int y0, int x1, int y1) {
        PostMessage(h, 0x0200, IntPtr.Zero, XY(x0, y0)); System.Threading.Thread.Sleep(60);
        PostMessage(h, 0x0201, (IntPtr)1, XY(x0, y0)); System.Threading.Thread.Sleep(80);
        for (int i = 1; i <= 8; i++) { PostMessage(h, 0x0200, (IntPtr)1, XY(x0 + (x1 - x0) * i / 8, y0 + (y1 - y0) * i / 8)); System.Threading.Thread.Sleep(30); }
        PostMessage(h, 0x0202, IntPtr.Zero, XY(x1, y1));
    }
    public static void Wheel(IntPtr h, int x, int y, int delta) {
        RECT r; GetWindowRect(h, out r);
        PostMessage(h, 0x020A, (IntPtr)(delta << 16), XY(r.L + x, r.T + y));
    }
    public static void Chars(IntPtr h, string text) { foreach (char ch in text) { PostMessage(h, 0x0102, (IntPtr)ch, (IntPtr)1); System.Threading.Thread.Sleep(15); } }
    public static void Key(IntPtr h, int vk) { PostMessage(h, 0x0100, (IntPtr)vk, (IntPtr)1); System.Threading.Thread.Sleep(30); PostMessage(h, 0x0101, (IntPtr)vk, (IntPtr)unchecked((int)0xC0000001)); }
}
"@
[JotWin]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
}
function Get-JotWindows([int]$ProcessId) {
    [JotWin]::Windows($ProcessId) | ForEach-Object {
        $r = [JotWin]::Rect($_)
        [pscustomobject]@{ Hwnd = $_; Title = [JotWin]::Title($_); X = $r[0]; Y = $r[1]; W = $r[2]; H = $r[3]; Dpi = [JotWin]::GetDpiForWindow($_) }
    }
}
# CLI: powershell.exe -File win.ps1 <cmd> <pid> [args]
#  list | cap <idx> <png> | click <idx> <x> <y> | drag <idx> <x1> <y1> <x2> <y2>
#  | key <idx> <vk> | chars <idx> <text> | size <idx> <w> <h> | tray <wm_command id>
if ($args.Count -ge 2) {
    $cmd = $args[0]; $wins = @(Get-JotWindows ([int]$args[1]))
    switch ($cmd) {
        'list'  { $i = 0; $wins | ForEach-Object { "$i`t$($_.Hwnd)`t$($_.X),$($_.Y) $($_.W)x$($_.H) dpi=$($_.Dpi)`t$($_.Title)"; $i++ } }
        'cap'   { [JotWin]::Capture($wins[[int]$args[2]].Hwnd, $args[3]); "saved $($args[3])" }
        'click' { [JotWin]::Click($wins[[int]$args[2]].Hwnd, [int]$args[3], [int]$args[4]) }
        'drag'  { [JotWin]::Drag($wins[[int]$args[2]].Hwnd, [int]$args[3], [int]$args[4], [int]$args[5], [int]$args[6]) }
        'key'   { [JotWin]::Key($wins[[int]$args[2]].Hwnd, [int]$args[3]) }
        'chars' { [JotWin]::Chars($wins[[int]$args[2]].Hwnd, [string]$args[3]) }
        'tray'  { $h = [JotWin]::TrayHost([uint32]$args[1]); if ($h -eq [IntPtr]::Zero) { throw 'no tray host' }; [JotWin]::PostMessage($h, 0x0111, [IntPtr][int]$args[2], [IntPtr]::Zero) | Out-Null; "posted $($args[2])" }
        'size'  { [JotWin]::SetWindowPos($wins[[int]$args[2]].Hwnd, [IntPtr]::Zero, 0, 0, [int]$args[3], [int]$args[4], 0x16) | Out-Null }
    }
}
