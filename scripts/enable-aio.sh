echo "Current values"
sysctl -a | grep aio
echo "Configuring new aio and file-max values"
sudo sysctl fs.aio-max-nr=1048576
sudo sysctl fs.file-max=1000000
echo "Save the following lines to /etc/sysctl.conf to persist changes"
echo "fs.aio-max-nr = 1048576"
echo "fs.file-max = 1000000"
echo "Run: sudo sysctl -p /etc/sysctl.conf to apply changes after modifying the file"
