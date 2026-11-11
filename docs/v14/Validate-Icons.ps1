param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [int]$ProcessId,
    [string]$ReportPath
)
$ErrorActionPreference='Stop'
$exe=(Resolve-Path -LiteralPath $Executable).Path
if(-not ('JotIconCheck' -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class JotIconCheck {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr LoadLibraryEx(string path, IntPtr file, uint flags);
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr FindResource(IntPtr module, IntPtr name, IntPtr type);
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr LockResource(IntPtr resource);
    [DllImport("kernel32.dll")] static extern bool FreeLibrary(IntPtr module);
    delegate bool EnumWindowProc(IntPtr hwnd, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowProc callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr hwnd, int index);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder title, int length);
    [DllImport("user32.dll")] static extern IntPtr SendMessage(IntPtr hwnd, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern int GetSystemMetricsForDpi(int index, uint dpi);
    [StructLayout(LayoutKind.Sequential)] struct IconInfo { public bool isIcon; public uint x; public uint y; public IntPtr mask; public IntPtr color; }
    [StructLayout(LayoutKind.Sequential)] struct Bitmap { public int type; public int width; public int height; public int widthBytes; public ushort planes; public ushort bitsPerPixel; public IntPtr bits; }
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetIconInfo(IntPtr icon, out IconInfo info);
    [DllImport("gdi32.dll")] static extern int GetObject(IntPtr handle, int bytes, out Bitmap bitmap);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr handle);
    public static int[] ResourceWidths(string path) {
        IntPtr module=LoadLibraryEx(path, IntPtr.Zero, 0x22);
        if(module==IntPtr.Zero) throw new Win32Exception();
        try {
            IntPtr group=FindResource(module, new IntPtr(1), new IntPtr(14));
            if(group==IntPtr.Zero) throw new Exception("Missing Jot RT_GROUP_ICON resource 1");
            IntPtr data=LockResource(LoadResource(module, group));
            if(data==IntPtr.Zero) throw new Win32Exception();
            int count=(ushort)Marshal.ReadInt16(data,4);
            List<int> widths=new List<int>();
            for(int i=0;i<count;i++) { int width=Marshal.ReadByte(data,6+i*14); widths.Add(width==0 ? 256 : width); }
            return widths.ToArray();
        } finally { FreeLibrary(module); }
    }
    static int IconWidth(IntPtr icon) {
        if(icon==IntPtr.Zero) return 0;
        IconInfo info;
        if(!GetIconInfo(icon,out info)) throw new Win32Exception();
        try {
            Bitmap bitmap;
            if(GetObject(info.color,Marshal.SizeOf<Bitmap>(),out bitmap)==0) throw new Win32Exception();
            return bitmap.width;
        } finally { if(info.color!=IntPtr.Zero) DeleteObject(info.color); if(info.mask!=IntPtr.Zero) DeleteObject(info.mask); }
    }
    public sealed class WindowResult {
        public long Hwnd; public string Title; public uint Dpi; public int SmallWidth; public int BigWidth;
        public int ExpectedSmallWidth; public int ExpectedBigWidth; public bool Pass;
    }
    public static WindowResult[] Windows(uint target) {
        List<WindowResult> rows=new List<WindowResult>();
        EnumWindows((hwnd,data)=> {
            uint pid; GetWindowThreadProcessId(hwnd,out pid);
            if(pid!=target || !IsWindowVisible(hwnd) || (GetWindowLongPtr(hwnd,-20).ToInt64() & 0x80)!=0) return true;
            StringBuilder title=new StringBuilder(256); GetWindowText(hwnd,title,title.Capacity);
            uint dpi=GetDpiForWindow(hwnd);
            int small=IconWidth(SendMessage(hwnd,0x7F,IntPtr.Zero,IntPtr.Zero));
            int big=IconWidth(SendMessage(hwnd,0x7F,new IntPtr(1),IntPtr.Zero));
            int expectedSmall=GetSystemMetricsForDpi(49,dpi), expectedBig=GetSystemMetricsForDpi(11,dpi);
            rows.Add(new WindowResult { Hwnd=hwnd.ToInt64(), Title=title.ToString(), Dpi=dpi, SmallWidth=small, BigWidth=big,
                ExpectedSmallWidth=expectedSmall, ExpectedBigWidth=expectedBig, Pass=small==expectedSmall && big==expectedBig && small>0 && big>0 });
            return true;
        },IntPtr.Zero);
        return rows.ToArray();
    }
}
"@
}
$widths=[JotIconCheck]::ResourceWidths($exe)
$missing=@(16,20,24,32,40,48,64,128,256 | Where-Object { $_ -notin $widths })
if($missing.Count){throw "Tailles absentes de la ressource ICO : $missing"}
$windows=@()
if($ProcessId) {
    $process=Get-Process -Id $ProcessId
    if($process.Path -ne $exe){throw "Le PID demandé n’utilise pas le binaire à vérifier."}
    $windows=@([JotIconCheck]::Windows($ProcessId))
    if(-not $windows.Count){throw 'Aucune fenêtre interactive visible pour ce PID. Ouvrir Jot avant le test.'}
    if(@($windows | Where-Object { -not $_.Pass }).Count){throw 'Une fenêtre ne présente pas ses deux icônes natives à la taille DPI attendue.'}
}
$result=[pscustomobject]@{
    Executable=$exe
    SHA256=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    ResourceWidths=$widths
    ProcessId=$ProcessId
    Windows=$windows
    Pass=$true
    Limitation='La visibilité et la netteté perçues dans la barre des tâches et Alt+Tab demandent aussi une observation visuelle.'
}
$json=$result | ConvertTo-Json -Depth 5
if($ReportPath){$json | Set-Content -LiteralPath $ReportPath -Encoding utf8}
$json
