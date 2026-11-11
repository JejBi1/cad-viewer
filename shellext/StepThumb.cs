// Explorer thumbnail provider for STEP / IGES / STL / DXF / DWG.
// The file stream is copied to a temp file and StepViewer.exe --thumbnail renders a PNG,
// which is returned to Explorer as a 32-bit ARGB bitmap. Windows caches the result itself.
//
// Build:  csc /target:library /platform:x64 /out:StepThumb.dll StepThumb.cs /r:System.Drawing.dll
// Register (per user): see installer\StepViewer.iss ([Registry] section "thumbs").
using System;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Threading;

[assembly: System.Reflection.AssemblyVersion("1.3.0.0")]

namespace StepThumb
{
    [ComImport, Guid("b824b49d-22ac-4161-ac8a-9916e8fa3f7f"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IInitializeWithStream
    {
        void Initialize(IStream pstream, uint grfMode);
    }

    [ComImport, Guid("b7d14566-0509-4cce-a71f-0a554233bd9b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IInitializeWithFile
    {
        void Initialize([MarshalAs(UnmanagedType.LPWStr)] string pszFilePath, uint grfMode);
    }

    [ComImport, Guid("e357fccd-a995-4576-b01f-234630154e96"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IThumbnailProvider
    {
        void GetThumbnail(uint cx, out IntPtr phbmp, out uint pdwAlpha);
    }

    [ComVisible(true), Guid("7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70"), ClassInterface(ClassInterfaceType.None)]
    public class StepThumbnailProvider : IInitializeWithFile, IInitializeWithStream, IThumbnailProvider
    {
        string filePath;     // set when initialized with a path (no copy needed)

        public void Initialize(string pszFilePath, uint grfMode)
        {
            Log("InitializeWithFile " + pszFilePath);
            filePath = pszFilePath;
        }

        const long MaxBytes = 80L * 1024 * 1024;      // bigger files: no thumbnail
        const int TimeoutMs = 60000;
        // NOTE (accepted): a named object in the session namespace can be created first by any process of the same
        // user session, e.g. with count 0, and then thumbnails time out ("busy") - denial of service of thumbnails
        // only, nothing is executed or read through it. Not worth a per-session ACL'd object.
        static readonly Semaphore Gate = new Semaphore(2, 2, "Local\\StepViewerThumbnails");

        IStream stream;

        public void Initialize(IStream pstream, uint grfMode)
        {
            Log("Initialize mode=" + grfMode);
            stream = pstream;
        }

        // diagnostics: create %TEMP%\StepViewerThumbs.log to enable
        internal static void Log(string msg)
        {
            try
            {
                string f = Path.Combine(Path.GetTempPath(), "StepViewerThumbs.log");
                if (File.Exists(f))
                    File.AppendAllText(f, DateTime.Now.ToString("HH:mm:ss.fff ") + msg + Environment.NewLine);
            }
            catch { }
        }

        public void GetThumbnail(uint cx, out IntPtr phbmp, out uint pdwAlpha)
        {
            phbmp = IntPtr.Zero;
            pdwAlpha = 2; // WTSAT_ARGB
            Log("GetThumbnail cx=" + cx);
            string dir = Sandbox.WorkDir();             // LocalLow: writable for the Low-integrity renderer
            string src = null, png = null;
            bool tempSrc = false;
            try
            {
                Directory.CreateDirectory(dir);
                string id = Guid.NewGuid().ToString("N");
                png = Path.Combine(dir, id + ".png");
                if (filePath != null)
                {
                    if (new FileInfo(filePath).Length > MaxBytes) throw new COMException("too big", unchecked((int)0x80004005));
                    src = filePath;          // real path: the viewer can also use its model cache
                }
                else
                {
                    System.Runtime.InteropServices.ComTypes.STATSTG st;
                    stream.Stat(out st, 0);
                    if (st.cbSize > MaxBytes) throw new COMException("too big", unchecked((int)0x80004005));
                    string ext = Path.GetExtension(st.pwcsName ?? "").ToLowerInvariant();
                    if (ext != ".step" && ext != ".stp" && ext != ".iges" && ext != ".igs" &&
                        ext != ".stl" && ext != ".dxf" && ext != ".dwg") ext = ".step";
                    src = Path.Combine(dir, id + ext);
                    tempSrc = true;
                    CopyStream(stream, src);
                }

                string exe = Path.Combine(Path.GetDirectoryName(typeof(StepThumbnailProvider).Assembly.Location),
                                          "StepViewer.exe");
                if (!Gate.WaitOne(TimeoutMs)) throw new COMException("busy", unchecked((int)0x80004005));
                try
                {
                    int code = Sandbox.Run(exe, "--thumbnail \"" + src + "\" \"" + png + "\" " +
                                           Math.Max(32u, Math.Min(cx, 1024u)), dir, TimeoutMs, Log);
                    if (code == -1)
                        throw new COMException("timeout", unchecked((int)0x80004005));
                    if (code != 0 || !File.Exists(png))
                        throw new COMException("render failed (" + code + ")", unchecked((int)0x80004005));
                }
                finally
                {
                    Gate.Release();
                }
                phbmp = LoadArgbBitmap(png, cx);
                Log("ok " + src);
            }
            catch (Exception ex)
            {
                Log("error: " + ex);
                throw;
            }
            finally
            {
                if (tempSrc) TryDelete(src);
                TryDelete(png);
                if (stream != null) { Marshal.ReleaseComObject(stream); stream = null; }
            }
        }

        static void CopyStream(IStream s, string path)
        {
            byte[] buf = new byte[1 << 20];
            IntPtr pcb = Marshal.AllocCoTaskMem(4);
            try
            {
                long total = 0;
                using (var fs = File.Create(path))
                {
                    while (true)
                    {
                        s.Read(buf, buf.Length, pcb);
                        int n = Marshal.ReadInt32(pcb);
                        if (n <= 0) break;
                        total += n;            // Stat() size can lie: the copy itself is capped too
                        if (total > MaxBytes) throw new COMException("too big", unchecked((int)0x80004005));
                        fs.Write(buf, 0, n);
                    }
                }
            }
            finally
            {
                Marshal.FreeCoTaskMem(pcb);
            }
        }

        const int MaxPngBytes = 4 * 1024 * 1024;       // a 1024 x 1024 thumbnail is a few hundred KB
        const int MaxPngDim = 2048;

        /// The PNG was written by the sandboxed (Low integrity) renderer, i.e. it is untrusted input for THIS
        /// (medium integrity, Explorer-hosted) process and GDI+ must not be pointed at the file: read it once into
        /// memory with a hard size cap (and delete it), check the PNG structure and the size from the bytes, and
        /// only then decode from the memory copy (no file that could change between the check and the decoding).
        static byte[] ReadPng(string path, int maxDim, out int width, out int height)
        {
            byte[] data;
            try
            {
                using (var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
                {
                    if (fs.Length > MaxPngBytes) throw new COMException("png too big", unchecked((int)0x80004005));
                    data = new byte[(int)fs.Length];
                    int got = 0;
                    while (got < data.Length)
                    {
                        int n = fs.Read(data, got, data.Length - got);
                        if (n <= 0) break;
                        got += n;
                    }
                    if (got != data.Length) throw new COMException("png truncated", unchecked((int)0x80004005));
                }
            }
            finally
            {
                TryDelete(path);
            }
            // signature, then chunks: IHDR (13 bytes) first, every chunk inside the file, IEND last
            byte[] sig = { 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A };
            if (data.Length < 8 + 25 + 12) throw new COMException("png too short", unchecked((int)0x80004005));
            for (int i = 0; i < 8; i++)
                if (data[i] != sig[i]) throw new COMException("not a png", unchecked((int)0x80004005));
            if (BE32(data, 8) != 13 || data[12] != 'I' || data[13] != 'H' || data[14] != 'D' || data[15] != 'R')
                throw new COMException("png: no IHDR", unchecked((int)0x80004005));
            uint w = BE32(data, 16), h = BE32(data, 20);
            if (w == 0 || h == 0 || w > (uint)maxDim || h > (uint)maxDim)
                throw new COMException("png: bad size " + w + "x" + h, unchecked((int)0x80004005));
            long pos = 8;
            bool end = false;
            while (pos + 12 <= data.Length && !end)
            {
                uint len = BE32(data, (int)pos);
                if (len > (uint)data.Length || pos + 12 + len > data.Length)
                    throw new COMException("png: chunk outside the file", unchecked((int)0x80004005));
                end = data[pos + 4] == 'I' && data[pos + 5] == 'E' && data[pos + 6] == 'N' && data[pos + 7] == 'D';
                pos += 12 + len;
            }
            if (!end) throw new COMException("png: no IEND", unchecked((int)0x80004005));
            width = (int)w;
            height = (int)h;
            return data;
        }

        static uint BE32(byte[] b, int o)
        {
            return ((uint)b[o] << 24) | ((uint)b[o + 1] << 16) | ((uint)b[o + 2] << 8) | b[o + 3];
        }

        static IntPtr LoadArgbBitmap(string png, uint cx)
        {
            int maxDim = (int)Math.Min((long)MaxPngDim, 2L * Math.Max(32u, Math.Min(cx, 1024u)));
            int pw, ph;
            byte[] bytes = ReadPng(png, maxDim, out pw, out ph);
            using (var ms = new MemoryStream(bytes, false))
            using (var src = new Bitmap(ms))
            {
                int w = src.Width, h = src.Height;
                if (w != pw || h != ph) throw new COMException("png: size differs from IHDR", unchecked((int)0x80004005));
                var bmi = new BITMAPINFOHEADER();
                bmi.biSize = (uint)Marshal.SizeOf(typeof(BITMAPINFOHEADER));
                bmi.biWidth = w;
                bmi.biHeight = -h;          // top-down
                bmi.biPlanes = 1;
                bmi.biBitCount = 32;
                IntPtr bits;
                IntPtr hbmp = CreateDIBSection(IntPtr.Zero, ref bmi, 0, out bits, IntPtr.Zero, 0);
                if (hbmp == IntPtr.Zero) throw new COMException("dib", unchecked((int)0x80004005));
                var data = src.LockBits(new Rectangle(0, 0, w, h), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                try
                {
                    byte[] row = new byte[w * 4];
                    for (int y = 0; y < h; y++)
                    {
                        Marshal.Copy(IntPtr.Add(data.Scan0, y * data.Stride), row, 0, row.Length);
                        Marshal.Copy(row, 0, IntPtr.Add(bits, y * w * 4), row.Length);
                    }
                }
                finally
                {
                    src.UnlockBits(data);
                }
                return hbmp;
            }
        }

        static void TryDelete(string p)
        {
            try { if (p != null && File.Exists(p)) File.Delete(p); } catch { }
        }

        [StructLayout(LayoutKind.Sequential)]
        struct BITMAPINFOHEADER
        {
            public uint biSize;
            public int biWidth;
            public int biHeight;
            public ushort biPlanes;
            public ushort biBitCount;
            public uint biCompression;
            public uint biSizeImage;
            public int biXPelsPerMeter;
            public int biYPelsPerMeter;
            public uint biClrUsed;
            public uint biClrImportant;
        }

        [DllImport("gdi32.dll", SetLastError = true)]
        static extern IntPtr CreateDIBSection(IntPtr hdc, ref BITMAPINFOHEADER pbmi, uint usage,
                                              out IntPtr ppvBits, IntPtr hSection, uint offset);
    }
}
