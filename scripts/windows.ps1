param([string]$Mode = "watch", [int]$Ms = 2500, [string]$Out = "")
Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<string> Snap(uint pid) {
    var o = new List<string>();
    var fg = GetForegroundWindow();
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) {
        RECT r; GetWindowRect(h, out r);
        var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
        if (r.R - r.L > 1)
          o.Add(String.Format("{0} [{1},{2} {3}x{4}]{5}", sb, r.L, r.T, r.R - r.L, r.B - r.T, h == fg ? " focused" : ""));
      }
      return true;
    }, IntPtr.Zero);
    return o;
  }
}
"@
[W]::SetProcessDPIAware() | Out-Null
$proc = Get-Process switchboard | Select-Object -First 1
if ($Mode -eq "watch") {
  # Prints each distinct set of visible windows with the time it first appeared
  $sw = [Diagnostics.Stopwatch]::StartNew(); $last = ""
  while ($sw.ElapsedMilliseconds -lt $Ms) {
    $now = ([W]::Snap($proc.Id) | Sort-Object) -join " | "
    if ($now -ne $last) { "{0,5}ms  {1}" -f $sw.ElapsedMilliseconds, $(if ($now) { $now } else { "(none)" }); $last = $now }
    Start-Sleep -Milliseconds 5
  }
} elseif ($Mode -eq "shot") {
  Add-Type -AssemblyName System.Windows.Forms, System.Drawing
  $b = [Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
  $g = [Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
  $bmp.Save($Out, [Drawing.Imaging.ImageFormat]::Png); "saved $Out ($($b.Width)x$($b.Height) at $($b.Left),$($b.Top))"
} else {
  [W]::Snap($proc.Id)
}
