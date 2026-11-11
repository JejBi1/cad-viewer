// Runs the thumbnail renderer sandboxed: Low integrity level (like browser sandboxes - it may read files but
// can write only to LocalLow, cannot touch the user's files / registry / other windows) inside a Job Object
// (memory cap, few processes, no clipboard / global UI, everything killed on timeout or when the job closes).
// Thumbnails parse untrusted files automatically (a downloaded drawing merely shown in a folder), so a parser
// bug in Open CASCADE / LibreDWG must not be able to do much.
using System;
using System.Collections;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace StepThumb
{
    static class Sandbox
    {
        public const long MemoryLimit = 2L * 1024 * 1024 * 1024;

        public static string WorkDir()
        {
            IntPtr p;
            Guid localLow = new Guid("A520A1A4-1780-4FF6-BD18-167343C5AF16");   // FOLDERID_LocalAppDataLow
            string root;
            if (SHGetKnownFolderPath(ref localLow, 0, IntPtr.Zero, out p) == 0)
            {
                root = Marshal.PtrToStringUni(p);
                Marshal.FreeCoTaskMem(p);
            }
            else
            {
                root = System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                                              "AppData", "LocalLow");
            }
            string dir = System.IO.Path.Combine(root, "StepViewerThumbs");
            System.IO.Directory.CreateDirectory(dir);     // inherits the Low integrity label of LocalLow
            return dir;
        }

        const int E_FAIL = unchecked((int)0x80004005);

        /// Returns the exit code, or -1 on timeout. FAILS CLOSED: the renderer is created suspended, with a Low
        /// integrity token, and is resumed only after it has been assigned to the job object. If any confinement
        /// step fails, the suspended process is terminated and an exception is thrown - an unconfined renderer
        /// never gets to run a single instruction (no thumbnail is better than an unsandboxed parser).
        public static int Run(string exe, string args, string workDir, int timeoutMs, Action<string> log)
        {
            IntPtr job = CreateJob(log);
            if (job == IntPtr.Zero)
                throw new COMException("sandbox: job object not available", E_FAIL);
            try
            {
                PROCESS_INFORMATION pi;
                if (!Start(exe, args, workDir, out pi, log))
                    throw new COMException("sandbox: Low integrity process not created", E_FAIL);
                bool resumed = false;
                try
                {
                    if (!AssignProcessToJobObject(job, pi.hProcess))
                    {
                        log("AssignProcessToJobObject failed " + Marshal.GetLastWin32Error());
                        throw new COMException("sandbox: process not assigned to the job", E_FAIL);
                    }
                    log("renderer: Low integrity + job");
                    if (ResumeThread(pi.hThread) == 0xFFFFFFFF)
                    {
                        log("ResumeThread failed " + Marshal.GetLastWin32Error());
                        throw new COMException("sandbox: thread not resumed", E_FAIL);
                    }
                    resumed = true;
                    uint w = WaitForSingleObject(pi.hProcess, (uint)timeoutMs);
                    if (w != 0)
                    {
                        TerminateJobObject(job, 1);
                        return -1;
                    }
                    uint code;
                    if (!GetExitCodeProcess(pi.hProcess, out code))
                        throw new COMException("sandbox: exit code unavailable", E_FAIL);
                    return (int)code;
                }
                finally
                {
                    // never leave a process that is not (yet) confined - or one we failed to start properly - running
                    if (!resumed && !TerminateProcess(pi.hProcess, 1))
                        log("TerminateProcess failed " + Marshal.GetLastWin32Error());
                    CloseHandle(pi.hThread);
                    CloseHandle(pi.hProcess);
                }
            }
            finally
            {
                CloseHandle(job);    // KILL_ON_JOB_CLOSE: nothing survives
            }
        }

        /// Job object with memory / process-count limits and UI restrictions; IntPtr.Zero if ANY step fails.
        static IntPtr CreateJob(Action<string> log)
        {
            IntPtr job = CreateJobObject(IntPtr.Zero, null);
            if (job == IntPtr.Zero)
            {
                log("CreateJobObject " + Marshal.GetLastWin32Error());
                return IntPtr.Zero;
            }
            bool ok = false;
            try
            {
                var info = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
                info.BasicLimitInformation.LimitFlags = 0x100 /*PROCESS_MEMORY*/ | 0x2000 /*KILL_ON_JOB_CLOSE*/ |
                                                        0x400 /*DIE_ON_UNHANDLED_EXCEPTION*/ | 0x8 /*ACTIVE_PROCESS*/;
                info.BasicLimitInformation.ActiveProcessLimit = 4;     // renderer + DWG converter
                info.ProcessMemoryLimit = new UIntPtr((ulong)MemoryLimit);
                int len = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
                IntPtr p = Marshal.AllocHGlobal(len);
                try
                {
                    Marshal.StructureToPtr(info, p, false);
                    if (!SetInformationJobObject(job, 9 /*ExtendedLimitInformation*/, p, (uint)len))
                    {
                        log("SetInformationJobObject(limits) " + Marshal.GetLastWin32Error());
                        return IntPtr.Zero;
                    }
                }
                finally { Marshal.FreeHGlobal(p); }
                // no clipboard, no global atoms / system parameters / display settings / desktops / exit windows,
                // no USER handles of processes outside the job
                var ui = new JOBOBJECT_BASIC_UI_RESTRICTIONS();
                ui.UIRestrictionsClass = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40 | 0x80;
                int ulen = Marshal.SizeOf(typeof(JOBOBJECT_BASIC_UI_RESTRICTIONS));
                IntPtr pu = Marshal.AllocHGlobal(ulen);
                try
                {
                    Marshal.StructureToPtr(ui, pu, false);
                    if (!SetInformationJobObject(job, 4 /*BasicUIRestrictions*/, pu, (uint)ulen))
                    {
                        log("SetInformationJobObject(ui) " + Marshal.GetLastWin32Error());
                        return IntPtr.Zero;
                    }
                }
                finally { Marshal.FreeHGlobal(pu); }
                ok = true;
                return job;
            }
            finally
            {
                if (!ok) CloseHandle(job);
            }
        }

        /// Creates the renderer SUSPENDED with a Low integrity primary token (copy of ours with the mandatory label
        /// lowered). No fallback to a normal token: false = nothing was started.
        static bool Start(string exe, string args, string workDir, out PROCESS_INFORMATION pi, Action<string> log)
        {
            pi = new PROCESS_INFORMATION();
            IntPtr token = IntPtr.Zero, dup = IntPtr.Zero, sid = IntPtr.Zero, env = IntPtr.Zero;
            try
            {
                var si = new STARTUPINFO();
                si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
                env = BuildEnvironment(workDir);
                var cmd = new StringBuilder("\"" + exe + "\" " + args);
                const uint flags = 0x08000000 /*NO_WINDOW*/ | 0x4 /*SUSPENDED*/ | 0x400 /*UNICODE_ENVIRONMENT*/;
                if (!OpenProcessToken(GetCurrentProcess(), 0x0002 | 0x0008 | 0x0080 | 0x0001, out token))
                {
                    log("OpenProcessToken " + Marshal.GetLastWin32Error());
                    return false;
                }
                if (!DuplicateTokenEx(token, 0x02000000 /*MAXIMUM_ALLOWED*/, IntPtr.Zero, 2, 1 /*primary*/, out dup))
                {
                    log("DuplicateTokenEx " + Marshal.GetLastWin32Error());
                    return false;
                }
                if (!ConvertStringSidToSid("S-1-16-4096", out sid))                 // Low mandatory level
                {
                    log("ConvertStringSidToSid " + Marshal.GetLastWin32Error());
                    return false;
                }
                var tml = new TOKEN_MANDATORY_LABEL();
                tml.Label.Sid = sid;
                tml.Label.Attributes = 0x20;                                        // SE_GROUP_INTEGRITY
                int size = Marshal.SizeOf(typeof(TOKEN_MANDATORY_LABEL)) + GetLengthSid(sid);
                if (!SetTokenInformation(dup, 25 /*TokenIntegrityLevel*/, ref tml, (uint)size))
                {
                    log("SetTokenInformation " + Marshal.GetLastWin32Error());
                    return false;
                }
                if (!CreateProcessAsUser(dup, exe, cmd, IntPtr.Zero, IntPtr.Zero, false, flags, env, workDir,
                                         ref si, out pi))
                {
                    log("CreateProcessAsUser " + Marshal.GetLastWin32Error());
                    return false;
                }
                return true;
            }
            finally
            {
                if (env != IntPtr.Zero) Marshal.FreeHGlobal(env);
                if (sid != IntPtr.Zero) LocalFree(sid);
                if (dup != IntPtr.Zero) CloseHandle(dup);
                if (token != IntPtr.Zero) CloseHandle(token);
            }
        }

        /// Current environment with temp / cache dirs moved to the Low-integrity work dir.
        static IntPtr BuildEnvironment(string workDir)
        {
            var vars = new SortedDictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            foreach (DictionaryEntry e in Environment.GetEnvironmentVariables())
                vars[(string)e.Key] = (string)e.Value;
            vars["TEMP"] = workDir;
            vars["TMP"] = workDir;
            vars["XDG_CACHE_HOME"] = workDir;      // ezdxf font cache
            vars["QT_QPA_PLATFORM"] = "offscreen";
            var sb = new StringBuilder();
            foreach (var kv in vars)
                sb.Append(kv.Key).Append('=').Append(kv.Value).Append('\0');
            sb.Append('\0');
            return Marshal.StringToHGlobalUni(sb.ToString());
        }

        // ---------------------------------------------------------------- P/Invoke
        [StructLayout(LayoutKind.Sequential)]
        struct SID_AND_ATTRIBUTES { public IntPtr Sid; public uint Attributes; }

        [StructLayout(LayoutKind.Sequential)]
        struct TOKEN_MANDATORY_LABEL { public SID_AND_ATTRIBUTES Label; }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        struct STARTUPINFO
        {
            public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
            public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
            public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }

        [StructLayout(LayoutKind.Sequential)]
        struct JOBOBJECT_BASIC_LIMIT_INFORMATION
        {
            public long PerProcessUserTimeLimit, PerJobUserTimeLimit; public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize; public uint ActiveProcessLimit;
            public UIntPtr Affinity; public uint PriorityClass, SchedulingClass;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct IO_COUNTERS
        {
            public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount,
                         ReadTransferCount, WriteTransferCount, OtherTransferCount;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
        {
            public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation; public IO_COUNTERS IoInfo;
            public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct JOBOBJECT_BASIC_UI_RESTRICTIONS { public uint UIRestrictionsClass; }

        [DllImport("shell32.dll")]
        static extern int SHGetKnownFolderPath(ref Guid id, uint flags, IntPtr token, out IntPtr path);
        [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr h);
        [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr h);
        [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr h);
        [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateProcess(IntPtr h, uint code);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern IntPtr CreateJobObject(IntPtr attrs, string name);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool SetInformationJobObject(IntPtr job, int cls, IntPtr info, uint len);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr proc);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateJobObject(IntPtr job, uint code);
        [DllImport("advapi32.dll", SetLastError = true)]
        static extern bool OpenProcessToken(IntPtr proc, uint access, out IntPtr token);
        [DllImport("advapi32.dll", SetLastError = true)]
        static extern bool DuplicateTokenEx(IntPtr token, uint access, IntPtr attrs, int level, int type, out IntPtr dup);
        [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern bool ConvertStringSidToSid(string sid, out IntPtr psid);
        [DllImport("advapi32.dll")] static extern int GetLengthSid(IntPtr sid);
        [DllImport("advapi32.dll", SetLastError = true)]
        static extern bool SetTokenInformation(IntPtr token, int cls, ref TOKEN_MANDATORY_LABEL info, uint len);
        [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern bool CreateProcessAsUser(IntPtr token, string app, StringBuilder cmd, IntPtr pattrs, IntPtr tattrs,
                                               bool inherit, uint flags, IntPtr env, string dir,
                                               ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    }
}
