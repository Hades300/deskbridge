using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.CompilerServices;
using System.Text.Json;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace DeskBridge.Admin;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        var model = new MainWindowModel();
        DataContext = model;
        Closed += (_, _) => model.Shutdown();
    }
}

public sealed class MainWindowModel : INotifyPropertyChanged
{
    private Process? _serverProcess;
    private string _statusText = "Stopped";
    private Brush _statusBrush = Brushes.Orange;
    private string _diagnostics = "No diagnostics yet.";
    private bool _captureInput = true;
    private string _serverName = "windows";
    private string _allowedClient = "mac";
    private string _clientPosition = "Right";

    public event PropertyChangedEventHandler? PropertyChanged;

    public string ListenAddress { get; set; } = "0.0.0.0:24800";

    public string ConfigPath { get; set; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "DeskBridge", "deskbridge.json");

    public bool CaptureInput
    {
        get => _captureInput;
        set => SetField(ref _captureInput, value);
    }

    public string ServerName
    {
        get => _serverName;
        set => SetField(ref _serverName, value);
    }

    public string AllowedClient
    {
        get => _allowedClient;
        set => SetField(ref _allowedClient, value);
    }

    public string ClientPosition
    {
        get => _clientPosition;
        set
        {
            if (SetField(ref _clientPosition, value))
            {
                OnPropertyChanged(nameof(LayoutArrow));
            }
        }
    }

    public string LayoutArrow => ClientPosition switch
    {
        "Left" => "<-",
        "Above" => "^",
        "Below" => "v",
        _ => "->",
    };

    public string StatusText
    {
        get => _statusText;
        private set => SetField(ref _statusText, value);
    }

    public Brush StatusBrush
    {
        get => _statusBrush;
        private set => SetField(ref _statusBrush, value);
    }

    public string Diagnostics
    {
        get => _diagnostics;
        private set => SetField(ref _diagnostics, value);
    }

    public ICommand StartCommand => new RelayCommand(Start);
    public ICommand StopCommand => new RelayCommand(Stop);
    public ICommand SaveConfigCommand => new RelayCommand(SaveConfig);
    public ICommand DiagnoseCommand => new RelayCommand(Diagnose);
    public ICommand FirewallCommand => new RelayCommand(OpenFirewall);

    public void Shutdown()
    {
        Stop();
    }

    private void Start()
    {
        Stop();
        SaveConfig();

        var daemonPath = LocateDaemon();
        if (!File.Exists(daemonPath))
        {
            StatusText = "Daemon missing";
            StatusBrush = Brushes.Red;
            Diagnostics =
                "Could not find deskbridge.exe next to the admin app.\n" +
                $"Expected: {daemonPath}\n\n" +
                "Use the Windows release zip as-is, or place deskbridge.exe in the same folder as DeskBridge.Admin.exe.";
            return;
        }

        var process = new Process
        {
            StartInfo = new ProcessStartInfo
            {
                FileName = daemonPath,
                Arguments = CaptureInput
                    ? $"server --config \"{ConfigPath}\" --capture"
                    : $"server --config \"{ConfigPath}\"",
                WorkingDirectory = AppContext.BaseDirectory,
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true,
            },
            EnableRaisingEvents = true,
        };
        process.OutputDataReceived += (_, args) => Append(args.Data);
        process.ErrorDataReceived += (_, args) => Append(args.Data);
        process.Exited += (_, _) =>
        {
            Application.Current.Dispatcher.Invoke(() =>
            {
                if (ReferenceEquals(_serverProcess, process))
                {
                    _serverProcess = null;
                }
                StatusText = "Stopped";
                StatusBrush = Brushes.Orange;
            });
        };

        try
        {
            process.Start();
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
            _serverProcess = process;
            StatusText = $"Running on {ListenAddress}";
            StatusBrush = Brushes.Green;
            Diagnostics =
                $"Started DeskBridge server.\nListen: {ListenAddress}\nScreen: {ServerName}\nAllowed client: {AllowedClient}\nDaemon: {daemonPath}";
        }
        catch (Exception ex)
        {
            StatusText = "Start failed";
            StatusBrush = Brushes.Red;
            Diagnostics = ex.ToString();
        }
    }

    private void Stop()
    {
        if (_serverProcess is { HasExited: false })
        {
            try
            {
                _serverProcess.Kill(entireProcessTree: true);
                _serverProcess.WaitForExit(2000);
            }
            catch (Exception ex)
            {
                Diagnostics = ex.ToString();
            }
        }
        _serverProcess = null;
        StatusText = "Stopped";
        StatusBrush = Brushes.Orange;
    }

    private void SaveConfig()
    {
        var edge = ClientPosition switch
        {
            "Left" => "left",
            "Above" => "top",
            "Below" => "bottom",
            _ => "right",
        };

        var config = new
        {
            server = new { name = ServerName, listen = ListenAddress },
            client = new { name = AllowedClient, server_addr = $"192.168.2.5:{ListenPort()}" },
            layout = new
            {
                screens = new object[]
                {
                    new { name = ServerName, size = new { width = 1920, height = 1080 } },
                    new { name = AllowedClient, size = new { width = 1728, height = 1117 } },
                },
                links = new object[] { new { from = ServerName, edge, to = AllowedClient } },
            },
            reliability = new { heartbeat_ms = 2000, reconnect_max_ms = 10000, stale_after_ms = 6000 },
        };

        var directory = Path.GetDirectoryName(ConfigPath);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var json = JsonSerializer.Serialize(config, new JsonSerializerOptions { WriteIndented = true });
        File.WriteAllText(ConfigPath, json);
        Diagnostics = $"Wrote config:\n{ConfigPath}";
    }

    private void Diagnose()
    {
        var port = ListenPort();
        Diagnostics =
            $"Status: {StatusText}\nTracked daemon: {DescribeTrackedDaemon()}\nServer: {ListenAddress}\nAllowed client: {AllowedClient}\nPosition: {ClientPosition}\n" +
            $"Daemon: {LocateDaemon()}\nDeskBridge processes:\n{DescribeDeskBridgeProcesses()}\n\n" +
            "From the Mac, run:\n" +
            $"deskbridge diag --server <WINDOWS_LAN_IP>:{port} --name mac\n\n" +
            "On Windows, verify the listener with:\n" +
            $"Get-NetTCPConnection -LocalPort {port} -State Listen\n\n" +
            "If logs show IHEL from 127.0.0.1, find the local process connecting to this port with:\n" +
            $"Get-NetTCPConnection -RemotePort {port} -State Established | Select-Object LocalAddress,LocalPort,OwningProcess";
    }

    private void OpenFirewall()
    {
        Diagnostics =
            "PowerShell firewall rule:\n" +
            $"New-NetFirewallRule -DisplayName \"DeskBridge TCP {ListenPort()}\" -Direction Inbound -Protocol TCP -LocalPort {ListenPort()} -Action Allow";
    }

    private void Append(string? line)
    {
        if (string.IsNullOrWhiteSpace(line)) return;
        Application.Current.Dispatcher.Invoke(() =>
        {
            var next = $"{Diagnostics}\n{line}";
            Diagnostics = next.Length > 20_000 ? next[^20_000..] : next;
        });
    }

    private static string LocateDaemon()
    {
        return Path.Combine(AppContext.BaseDirectory, "deskbridge.exe");
    }

    private string ListenPort()
    {
        var lastColon = ListenAddress.LastIndexOf(':');
        if (lastColon >= 0 && lastColon < ListenAddress.Length - 1)
        {
            return ListenAddress[(lastColon + 1)..];
        }

        return "24800";
    }

    private string DescribeTrackedDaemon()
    {
        return _serverProcess switch
        {
            null => "not tracked by this window",
            { HasExited: false } process => $"running, pid {process.Id}",
            { HasExited: true } process => $"exited, code {process.ExitCode}",
        };
    }

    private static string DescribeDeskBridgeProcesses()
    {
        var descriptions = Process.GetProcessesByName("deskbridge")
            .Select(process =>
            {
                try
                {
                    return $"pid {process.Id}: {process.MainModule?.FileName ?? process.ProcessName}";
                }
                catch
                {
                    return $"pid {process.Id}: {process.ProcessName}";
                }
            })
            .ToArray();

        return descriptions.Length == 0 ? "none found" : string.Join("\n", descriptions);
    }

    private bool SetField<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(name);
        return true;
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
    }
}

public sealed class RelayCommand(Action execute) : ICommand
{
    public event EventHandler? CanExecuteChanged
    {
        add { }
        remove { }
    }

    public bool CanExecute(object? parameter) => true;
    public void Execute(object? parameter) => execute();
}
