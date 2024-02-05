#!/usr/bin/env python3
import boto3
from concurrent.futures import ThreadPoolExecutor

def get_ipv6_addresses(instance):
    network_interfaces = instance.get('NetworkInterfaces', [])

    ipv6_addresses = []
    for interface in network_interfaces:
        ipv6_addresses.extend(addr.get('Ipv6Address', 'N/A') for addr in interface.get('Ipv6Addresses', []))

    return ', '.join(ipv6_addresses) if ipv6_addresses else None

def get_instance_info(region):
    ec2 = boto3.client('ec2', region_name=region)
    instances = ec2.describe_instances()
    print(20*"=", f"region: {region}")
    
    for reservation in instances['Reservations']:
        for instance in reservation['Instances']:
            try:
                instance_id = instance['InstanceId']
                state = instance['State']['Name']
                public_ip = instance.get('PublicIpAddress', 'N/A')
                ipv6_addresses = get_ipv6_addresses(instance)
                core_count = instance['CpuOptions']['CoreCount']
                name_tag = next((tag['Value'] for tag in instance.get('Tags', []) if tag['Key'] == 'Name'), 'N/A')
            
                print(f"{instance_id}, State: {state} Name: {name_tag} ({core_count} cpu cores)")
                print(f"    Public IP: {public_ip}")
                if ipv6_addresses:
                    print(f"    IPv6: {ipv6_addresses}")
            except Exception as e:
                print(f"An error occurred: {e}, root cause: {e.args[0]}")

regions = ['us-east-1', 'il-central-1']

with ThreadPoolExecutor() as executor:
    # Execute the get_instance_info function for each region concurrently
    executor.map(get_instance_info, regions)
