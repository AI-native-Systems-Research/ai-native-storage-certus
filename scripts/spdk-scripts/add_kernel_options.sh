# For Intel machine
#
grubby --update-kernel=ALL --args="intel_iommu=on iommu=pt default_hugepagesz=1G hugepagesz=1G hugepages=32"

# Check
grubby --info=ALL
